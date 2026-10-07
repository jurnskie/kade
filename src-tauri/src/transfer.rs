//! Upload/download queue. Each top-level item the user drops becomes one job;
//! directories are walked and copied recursively. Jobs run a few at a time on
//! the shared SFTP session, each copying several files at once, and report
//! progress through the `transfer` event.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{watch, Semaphore};

use crate::backup::{self, Recorder};
use crate::error::{AppError, AppResult};
use crate::remote::{Meta, RemoteFs};
use crate::ssh::{join_remote, Session};

const CHUNK: usize = 256 * 1024;
const PARALLEL_JOBS: usize = 3;
const EMIT_EVERY: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Upload,
    Download,
}

/// What to do when the destination already exists.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Conflict {
    Overwrite,
    Skip,
    /// Only copy when the source is newer than the destination.
    Newer,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Scanning,
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub id: String,
    pub session_id: String,
    pub name: String,
    pub direction: Direction,
    pub dest: String,
    pub state: JobState,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub files_done: u64,
    pub files_total: u64,
    pub skipped: u64,
    /// Bytes per second, smoothed.
    pub speed: f64,
    pub error: Option<String>,
    /// Backup transaction holding the files this job overwrote.
    pub backup_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Flags {
    paused: bool,
    cancelled: bool,
}

/// Pause/cancel switches of one job. A watch channel, so a waiter can't miss
/// a resume that lands between checking the flags and starting to wait.
struct Ctl {
    flags: watch::Sender<Flags>,
}

impl Default for Ctl {
    fn default() -> Self {
        Ctl { flags: watch::Sender::new(Flags::default()) }
    }
}

impl Ctl {
    fn cancelled(&self) -> bool {
        self.flags.borrow().cancelled
    }

    /// Waits while paused; errors once cancelled.
    async fn checkpoint(&self) -> AppResult<()> {
        // Checks the current flags before waiting. The sender lives in `self`,
        // so the channel can't close.
        let mut rx = self.flags.subscribe();
        let cancelled = rx.wait_for(|f| !f.paused || f.cancelled).await.map_or(true, |f| f.cancelled);
        if cancelled {
            return Err(AppError::other(tr!("cancelled", "geannuleerd")));
        }
        Ok(())
    }

    /// Mirrors pause and resume into the job's state. Never finishes; race it
    /// against the job.
    async fn report_pauses(&self, rep: &SharedReporter) -> Infallible {
        let mut rx = self.flags.subscribe();
        loop {
            let paused = rx.borrow_and_update().paused;
            rep.lock().unwrap().set_paused(paused);
            if rx.changed().await.is_err() {
                return std::future::pending().await;
            }
        }
    }
}

pub struct Transfers {
    jobs: Mutex<HashMap<String, Arc<Ctl>>>,
    slots: Arc<Semaphore>,
}

impl Default for Transfers {
    fn default() -> Self {
        Transfers { jobs: Mutex::default(), slots: Arc::new(Semaphore::new(PARALLEL_JOBS)) }
    }
}

impl Transfers {
    fn ctl(&self, id: &str) -> Option<Arc<Ctl>> {
        self.jobs.lock().unwrap().get(id).cloned()
    }

    pub fn pause(&self, id: &str, paused: bool) {
        if let Some(ctl) = self.ctl(id) {
            ctl.flags.send_if_modified(|f| std::mem::replace(&mut f.paused, paused) != paused);
        }
    }

    pub fn cancel(&self, id: &str) {
        if let Some(ctl) = self.ctl(id) {
            ctl.flags.send_modify(|f| f.cancelled = true);
        }
    }
}

fn basename(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string()
}

fn unix_secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Top-level names in `sources` that already exist in `dest_dir`.
pub async fn conflicts(session: &Session, direction: Direction, sources: &[String], dest_dir: &str) -> AppResult<Vec<String>> {
    let mut out = Vec::new();
    for src in sources {
        let name = basename(src);
        let exists = match direction {
            Direction::Upload => session.fs()?.exists(&join_remote(dest_dir, &name)).await?,
            Direction::Download => Path::new(dest_dir).join(&name).exists(),
        };
        if exists {
            out.push(name);
        }
    }
    Ok(out)
}

/// One file to copy (or directory to create) after scanning.
struct Item {
    src: String,
    dst: String,
    is_dir: bool,
    size: u64,
    mtime: i64,
}

fn scan_local(root: &str, dest_root: &str) -> AppResult<Vec<Item>> {
    let mut items = Vec::new();
    let mut stack = vec![(PathBuf::from(root), dest_root.to_string())];
    while let Some((src, dst)) = stack.pop() {
        let meta = std::fs::metadata(&src)?;
        let mtime = meta.modified().map(unix_secs).unwrap_or(0);
        if meta.is_dir() {
            items.push(Item { src: src.to_string_lossy().into(), dst: dst.clone(), is_dir: true, size: 0, mtime });
            for entry in std::fs::read_dir(&src)? {
                let entry = entry?;
                // Don't descend into symlinked directories (loops, surprises).
                if entry.file_type()?.is_symlink() && entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                stack.push((entry.path(), join_remote(&dst, &name)));
            }
        } else {
            items.push(Item { src: src.to_string_lossy().into(), dst, is_dir: false, size: meta.len(), mtime });
        }
    }
    Ok(items)
}

async fn scan_remote(session: &Session, root: &str, dest_root: &Path) -> AppResult<Vec<Item>> {
    let fs = session.fs()?;
    let meta = fs.stat(root).await?.ok_or_else(|| AppError::other(tr!("{root} doesn't exist", "{root} bestaat niet", root = root)))?;
    let mut items = Vec::new();
    if !meta.is_dir {
        items.push(Item {
            src: root.to_string(),
            dst: dest_root.to_string_lossy().into(),
            is_dir: false,
            size: meta.size,
            mtime: meta.mtime.unwrap_or(0),
        });
        return Ok(items);
    }
    // Walk with listings only: one LIST per folder, which matters a lot on FTP.
    let mut stack = vec![(root.to_string(), dest_root.to_path_buf(), meta.mtime.unwrap_or(0))];
    while let Some((src, dst, mtime)) = stack.pop() {
        items.push(Item { src: src.clone(), dst: dst.to_string_lossy().into(), is_dir: true, size: 0, mtime });
        for entry in fs.list(&src).await? {
            let child_dst = dst.join(&entry.name);
            let mtime = entry.modified.unwrap_or(0);
            if entry.is_dir {
                // Don't descend into symlinked directories (loops, surprises).
                if !entry.is_symlink {
                    stack.push((entry.path, child_dst, mtime));
                }
            } else {
                items.push(Item { src: entry.path, dst: child_dst.to_string_lossy().into(), is_dir: false, size: entry.size, mtime });
            }
        }
    }
    Ok(items)
}

/// Receives every progress update (the app forwards them as `transfer` events).
pub type Emit = Arc<dyn Fn(&Progress) + Send + Sync>;

struct Reporter {
    emit: Emit,
    progress: Progress,
    /// Where the job is; `progress.state` shows Paused instead while paused.
    phase: JobState,
    paused: bool,
    last_emit: Instant,
    window_start: Instant,
    window_bytes: u64,
}

impl Reporter {
    fn emit(&mut self) {
        self.last_emit = Instant::now();
        (self.emit)(&self.progress);
    }

    fn state(&mut self, state: JobState) {
        self.phase = state;
        self.show_state();
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.show_state();
    }

    fn show_state(&mut self) {
        self.progress.state = match self.phase {
            JobState::Queued | JobState::Scanning | JobState::Running if self.paused => JobState::Paused,
            phase => phase,
        };
        self.emit();
    }

    fn add_bytes(&mut self, n: u64) {
        self.progress.bytes_done += n;
        self.window_bytes += n;
        let elapsed = self.window_start.elapsed();
        if elapsed >= Duration::from_millis(500) {
            let now = self.window_bytes as f64 / elapsed.as_secs_f64();
            self.progress.speed = if self.progress.speed == 0.0 { now } else { self.progress.speed * 0.6 + now * 0.4 };
            self.window_start = Instant::now();
            self.window_bytes = 0;
        }
        if self.last_emit.elapsed() >= EMIT_EVERY {
            self.emit();
        }
    }
}

/// What an upload knows about the destination tree. Each folder is listed
/// once, instead of a STAT (on FTP: a whole LIST of the parent) per file.
#[derive(Default)]
struct Listings {
    /// Remote folders whose complete contents are in `entries`.
    dirs: HashSet<String>,
    entries: HashMap<String, Meta>,
}

impl Listings {
    async fn add_dir(&mut self, fs: &RemoteFs, dir: &str) -> AppResult<()> {
        for entry in fs.list(dir).await? {
            let meta = Meta { is_dir: entry.is_dir, is_symlink: entry.is_symlink, size: entry.size, mtime: entry.modified };
            self.entries.insert(entry.path, meta);
        }
        self.dirs.insert(dir.to_string());
        Ok(())
    }

    /// Metadata of `path`, from the listings when its folder was listed.
    async fn stat(&self, fs: &RemoteFs, path: &str) -> AppResult<Option<Meta>> {
        if let Some(meta) = self.entries.get(path) {
            return Ok(Some(meta.clone()));
        }
        if self.dirs.contains(backup::parent(path)) {
            return Ok(None);
        }
        fs.stat(path).await
    }
}

enum Decision {
    Skip,
    /// Copy; `replaces` means an existing destination gets backed up first.
    Copy {
        replaces: bool,
    },
}

async fn decide(session: &Session, direction: Direction, item: &Item, policy: Conflict, listings: &Listings) -> AppResult<Decision> {
    let dst_mtime = match direction {
        Direction::Upload => match listings.stat(session.fs()?, &item.dst).await? {
            Some(m) => m.mtime,
            None => return Ok(Decision::Copy { replaces: false }),
        },
        Direction::Download => match std::fs::metadata(&item.dst) {
            Ok(m) => m.modified().ok().map(unix_secs),
            Err(_) => return Ok(Decision::Copy { replaces: false }),
        },
    };
    let tolerance = session.fs()?.mtime_tolerance();
    let copy = match policy {
        Conflict::Overwrite => true,
        Conflict::Skip => false,
        // Remote times can be coarse (FTP LIST: minutes), so allow that much slack.
        Conflict::Newer => dst_mtime.is_none_or(|d| item.mtime > d + tolerance),
    };
    Ok(if copy { Decision::Copy { replaces: true } } else { Decision::Skip })
}

/// The job's reporter, shared by the files that copy at the same time.
type SharedReporter = Mutex<Reporter>;

async fn copy_file(session: &Session, direction: Direction, item: &Item, ctl: &Ctl, rep: &SharedReporter) -> AppResult<()> {
    let fs = session.fs()?;
    let mut buf = vec![0u8; CHUNK];
    match direction {
        Direction::Upload => {
            let mut src = tokio::fs::File::open(&item.src).await?;
            let mut dst = fs.writer(&item.dst).await?;
            loop {
                ctl.checkpoint().await?;
                let n = src.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                dst.write_all(&buf[..n]).await?;
                rep.lock().unwrap().add_bytes(n as u64);
            }
            dst.finish().await?;
            // Keep the source's modification time, so "only newer" works both ways.
            fs.set_mtime(&item.dst, item.mtime).await;
        }
        Direction::Download => {
            let mut src = fs.reader(&item.src).await?;
            let mut dst = tokio::fs::File::create(&item.dst).await?;
            loop {
                ctl.checkpoint().await?;
                let n = src.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                dst.write_all(&buf[..n]).await?;
                rep.lock().unwrap().add_bytes(n as u64);
            }
            src.finish().await?;
            dst.flush().await?;
            let dst = dst.into_std().await;
            let _ = dst.set_modified(UNIX_EPOCH + Duration::from_secs(item.mtime.max(0) as u64));
        }
    }
    Ok(())
}

/// Create the job's directories, parents first. For uploads, also list the
/// ones that already exist so the files can be checked without a request each.
async fn make_dirs(session: &Session, direction: Direction, items: &[Item], ctl: &Ctl) -> AppResult<Listings> {
    let mut listings = Listings::default();
    for item in items.iter().filter(|i| i.is_dir) {
        ctl.checkpoint().await?;
        match direction {
            Direction::Upload => {
                let fs = session.fs()?;
                if listings.stat(fs, &item.dst).await?.is_some() {
                    listings.add_dir(fs, &item.dst).await?;
                } else {
                    fs.mkdir(&item.dst).await?;
                    listings.dirs.insert(item.dst.clone());
                }
            }
            Direction::Download => std::fs::create_dir_all(&item.dst)?,
        }
    }
    Ok(listings)
}

#[allow(clippy::too_many_arguments)]
async fn transfer_file(
    session: &Session,
    direction: Direction,
    item: &Item,
    policy: Conflict,
    listings: &Listings,
    ctl: &Ctl,
    rep: &SharedReporter,
    undo: &tokio::sync::Mutex<&mut Recorder>,
) -> AppResult<()> {
    ctl.checkpoint().await?;
    let side = match direction {
        Direction::Upload => Some(session),
        Direction::Download => None,
    };
    let replaces = match decide(session, direction, item, policy, listings).await? {
        Decision::Skip => {
            let mut rep = rep.lock().unwrap();
            rep.progress.skipped += 1;
            rep.progress.bytes_total -= item.size;
            return Ok(());
        }
        Decision::Copy { replaces } => replaces,
    };
    if replaces {
        let mut undo = undo.lock().await;
        undo.stash(side, &item.dst).await?;
        rep.lock().unwrap().progress.backup_id = Some(undo.id().to_string());
    }
    if let Err(e) = copy_file(session, direction, item, ctl, rep).await {
        // Don't leave a half-written file behind.
        match direction {
            Direction::Upload => {
                let _ = session.fs()?.remove_file(&item.dst).await;
            }
            Direction::Download => {
                let _ = std::fs::remove_file(&item.dst);
            }
        }
        // And put back the file it was replacing.
        if replaces {
            let mut undo = undo.lock().await;
            match undo.put_back(side, &item.dst).await {
                Ok(()) if undo.is_empty() => rep.lock().unwrap().progress.backup_id = None,
                Ok(()) => {}
                Err(err) => eprintln!("kade: restoring {} failed, it stays in the backup: {err}", item.dst),
            }
        }
        return Err(e);
    }
    let mut rep = rep.lock().unwrap();
    rep.progress.files_done += 1;
    rep.emit();
    Ok(())
}

async fn run_job(
    session: &Session,
    direction: Direction,
    source: &str,
    policy: Conflict,
    ctl: &Ctl,
    rep: &SharedReporter,
    undo: &mut Recorder,
) -> AppResult<()> {
    let dest = {
        let mut rep = rep.lock().unwrap();
        rep.state(JobState::Scanning);
        rep.progress.dest.clone()
    };
    let items = match direction {
        Direction::Upload => scan_local(source, &dest)?,
        Direction::Download => scan_remote(session, source, Path::new(&dest)).await?,
    };
    {
        let mut rep = rep.lock().unwrap();
        rep.progress.files_total = items.iter().filter(|i| !i.is_dir).count() as u64;
        rep.progress.bytes_total = items.iter().map(|i| i.size).sum();
        rep.state(JobState::Running);
    }

    // Scanning pushes parents before children, so this creates them in order.
    let listings = make_dirs(session, direction, &items, ctl).await?;

    // Small files are dominated by round trips, so copy several at once.
    let parallel = session.fs()?.parallel_files();
    let undo = tokio::sync::Mutex::new(undo);
    let mut files = items.iter().filter(|i| !i.is_dir);
    let mut running = FuturesUnordered::new();
    let mut failure = None;
    loop {
        while failure.is_none() && running.len() < parallel {
            let Some(item) = files.next() else { break };
            running.push(transfer_file(session, direction, item, policy, &listings, ctl, rep, &undo));
        }
        // After a failure, let the files in flight finish (or clean up), then report the first error.
        match running.next().await {
            Some(Err(e)) => {
                failure.get_or_insert(e);
            }
            Some(Ok(())) => {}
            None => break,
        }
    }
    failure.map_or(Ok(()), Err)
}

/// Queue one job per source. Returns the job ids immediately.
pub fn start(
    emit: Emit,
    transfers: Arc<Transfers>,
    session: Arc<Session>,
    session_id: String,
    direction: Direction,
    sources: Vec<String>,
    dest_dir: String,
    policy: Conflict,
) -> Vec<String> {
    let mut ids = Vec::new();
    for source in sources {
        let id = uuid::Uuid::new_v4().to_string();
        let name = basename(&source);
        let dest = match direction {
            Direction::Upload => join_remote(&dest_dir, &name),
            Direction::Download => Path::new(&dest_dir).join(&name).to_string_lossy().into_owned(),
        };
        let ctl = Arc::new(Ctl::default());
        transfers.jobs.lock().unwrap().insert(id.clone(), ctl.clone());

        let mut rep = Reporter {
            emit: emit.clone(),
            phase: JobState::Queued,
            paused: false,
            progress: Progress {
                id: id.clone(),
                session_id: session_id.clone(),
                name,
                direction,
                dest,
                state: JobState::Queued,
                bytes_done: 0,
                bytes_total: 0,
                files_done: 0,
                files_total: 0,
                skipped: 0,
                speed: 0.0,
                error: None,
                backup_id: None,
            },
            last_emit: Instant::now(),
            window_start: Instant::now(),
            window_bytes: 0,
        };
        rep.emit();

        let session = session.clone();
        let transfers = transfers.clone();
        tauri::async_runtime::spawn(async move {
            let rep = Mutex::new(rep);
            // Pausing works from the queue on; the job stops at its first checkpoint.
            let result = tokio::select! {
                done = run_queued(&transfers, &session, direction, &source, policy, &ctl, &rep) => done,
                never = ctl.report_pauses(&rep) => match never {},
            };
            let mut rep = rep.into_inner().unwrap();
            rep.progress.speed = 0.0;
            match result {
                Ok(()) => rep.state(JobState::Done),
                Err(_) if ctl.cancelled() => rep.state(JobState::Cancelled),
                Err(e) => {
                    rep.progress.error = Some(e.to_string());
                    rep.state(JobState::Failed);
                }
            }
            transfers.jobs.lock().unwrap().remove(&rep.progress.id);
        });
        ids.push(id);
    }
    ids
}

/// Wait for a job slot, then run the job and record what it overwrote.
async fn run_queued(
    transfers: &Transfers,
    session: &Session,
    direction: Direction,
    source: &str,
    policy: Conflict,
    ctl: &Ctl,
    rep: &SharedReporter,
) -> AppResult<()> {
    let _slot = transfers.slots.acquire().await;
    let (name, dest) = {
        let rep = rep.lock().unwrap();
        (rep.progress.name.clone(), rep.progress.dest.clone())
    };
    let side = match direction {
        Direction::Upload => backup::Side::Remote,
        Direction::Download => backup::Side::Local,
    };
    let mut undo = Recorder::new(
        side,
        backup::Op::Overwrite,
        (direction == Direction::Upload).then_some(session),
        match direction {
            Direction::Upload => {
                tr!("Upload of {} overwrote files in {}", "Upload van {} overschreef bestanden in {}", name, dest)
            }
            Direction::Download => {
                tr!("Download of {} overwrote files in {}", "Download van {} overschreef bestanden in {}", name, dest)
            }
        },
    );
    let result = run_job(session, direction, source, policy, ctl, rep, &mut undo).await;
    // Record the backup even after a failure: what was overwritten so far is in it.
    if let Err(e) = undo.commit() {
        eprintln!("kade: backup-index bijwerken mislukt: {e}");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reporter(states: Arc<Mutex<Vec<JobState>>>) -> Reporter {
        Reporter {
            emit: Arc::new(move |p: &Progress| states.lock().unwrap().push(p.state)),
            phase: JobState::Queued,
            paused: false,
            progress: Progress {
                id: "j".into(),
                session_id: "s".into(),
                name: "x".into(),
                direction: Direction::Upload,
                dest: "/x".into(),
                state: JobState::Queued,
                bytes_done: 0,
                bytes_total: 0,
                files_done: 0,
                files_total: 0,
                skipped: 0,
                speed: 0.0,
                error: None,
                backup_id: None,
            },
            last_emit: Instant::now(),
            window_start: Instant::now(),
            window_bytes: 0,
        }
    }

    #[tokio::test]
    async fn checkpoint_waits_until_resumed_or_cancelled() {
        let ctl = Arc::new(Ctl::default());
        assert!(ctl.checkpoint().await.is_ok());

        ctl.flags.send_modify(|f| f.paused = true);
        let waiter = tokio::spawn({
            let ctl = ctl.clone();
            async move { ctl.checkpoint().await.is_ok() }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "a paused job must wait");
        ctl.flags.send_modify(|f| f.paused = false);
        assert!(waiter.await.unwrap());

        ctl.flags.send_modify(|f| f.paused = true);
        let waiter = tokio::spawn({
            let ctl = ctl.clone();
            async move { ctl.checkpoint().await.is_ok() }
        });
        ctl.flags.send_modify(|f| f.cancelled = true);
        assert!(!waiter.await.unwrap(), "cancelling ends the wait with an error");
    }

    /// Pausing shows up in the job state while files are in flight, and resuming switches it back.
    #[tokio::test]
    async fn pause_and_resume_are_reported() {
        let states = Arc::new(Mutex::new(Vec::new()));
        let rep = Mutex::new(reporter(states.clone()));
        rep.lock().unwrap().state(JobState::Running);
        let ctl = Ctl::default();
        let job = async {
            ctl.flags.send_modify(|f| f.paused = true);
            tokio::task::yield_now().await;
            let resume = async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                ctl.flags.send_modify(|f| f.paused = false);
            };
            let (r, ()) = tokio::join!(ctl.checkpoint(), resume);
            tokio::task::yield_now().await;
            r
        };
        let result = tokio::select! {
            r = job => r,
            never = ctl.report_pauses(&rep) => match never {},
        };
        assert!(result.is_ok());
        let states = states.lock().unwrap().clone();
        assert!(states.ends_with(&[JobState::Paused, JobState::Running]), "{states:?}");
    }
}
