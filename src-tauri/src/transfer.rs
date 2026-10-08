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
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Semaphore};

use crate::backup::{self, Recorder};
use crate::error::{AppError, AppResult};
use crate::remote::{copy_chunks, Kept, Meta, RemoteFs};
use crate::ssh::{join_remote, Session};

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

/// What kind of job a queue entry is. Sync jobs can't be paused.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    #[default]
    Transfer,
    Sync,
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
    pub kind: JobKind,
    /// Something the user should know about a job that still finished.
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Flags {
    paused: bool,
    cancelled: bool,
}

/// Pause/cancel switches of one job. A watch channel, so a waiter can't miss
/// a resume that lands between checking the flags and starting to wait.
pub(crate) struct Ctl {
    flags: watch::Sender<Flags>,
    pausable: bool,
}

impl Default for Ctl {
    fn default() -> Self {
        Ctl { flags: watch::Sender::new(Flags::default()), pausable: true }
    }
}

impl Ctl {
    pub(crate) fn cancelled(&self) -> bool {
        self.flags.borrow().cancelled
    }

    /// Finishes once the job is cancelled.
    pub(crate) async fn until_cancelled(&self) {
        let _ = self.flags.subscribe().wait_for(|f| f.cancelled).await;
    }

    /// Waits while paused; errors once cancelled.
    pub(crate) async fn checkpoint(&self) -> AppResult<()> {
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
    pub(crate) slots: Arc<Semaphore>,
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

    /// A job that can't be paused (a sync) ignores this.
    pub fn pause(&self, id: &str, paused: bool) {
        if let Some(ctl) = self.ctl(id).filter(|c| c.pausable) {
            ctl.flags.send_if_modified(|f| std::mem::replace(&mut f.paused, paused) != paused);
        }
    }

    pub fn cancel(&self, id: &str) {
        if let Some(ctl) = self.ctl(id) {
            ctl.flags.send_modify(|f| f.cancelled = true);
        }
    }

    /// Add a job that can be cancelled (and paused, when `pausable`) by id.
    pub(crate) fn register(&self, id: &str, pausable: bool) -> Arc<Ctl> {
        let ctl = Arc::new(Ctl { pausable, ..Ctl::default() });
        self.jobs.lock().unwrap().insert(id.to_string(), ctl.clone());
        ctl
    }

    pub(crate) fn finish(&self, id: &str) {
        self.jobs.lock().unwrap().remove(id);
    }
}

pub(crate) fn basename(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string()
}

fn unix_secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Top-level names in `sources` that already exist in `dest_dir`.
pub async fn conflicts(session: &Session, direction: Direction, sources: &[String], dest_dir: &str) -> AppResult<Vec<String>> {
    let names = sources.iter().map(|s| basename(s));
    Ok(match direction {
        // One listing instead of a request per name (on FTP each is a LIST anyway).
        Direction::Upload => {
            let existing: HashSet<String> = session.fs()?.list(dest_dir).await?.into_iter().map(|e| e.name).collect();
            names.filter(|n| existing.contains(n)).collect()
        }
        Direction::Download => names.filter(|n| Path::new(dest_dir).join(n).exists()).collect(),
    })
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

async fn scan_remote(session: &Session, ctl: &Ctl, root: &str, dest_root: &Path) -> AppResult<Vec<Item>> {
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
        // A big tree takes a while to list; cancelling or pausing shouldn't wait for all of it.
        ctl.checkpoint().await?;
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

pub(crate) struct Reporter {
    emit: Emit,
    pub(crate) progress: Progress,
    /// Where the job is; `progress.state` shows Paused instead while paused.
    phase: JobState,
    paused: bool,
    last_emit: Instant,
    window_start: Instant,
    window_bytes: u64,
}

impl Reporter {
    pub(crate) fn new(emit: Emit, progress: Progress) -> Self {
        let phase = progress.state;
        Reporter { emit, progress, phase, paused: false, last_emit: Instant::now(), window_start: Instant::now(), window_bytes: 0 }
    }

    pub(crate) fn emit(&mut self) {
        self.last_emit = Instant::now();
        (self.emit)(&self.progress);
    }

    pub(crate) fn state(&mut self, state: JobState) {
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
        self.sample_speed(n);
    }

    /// Emit, at most every `EMIT_EVERY`.
    pub(crate) fn tick(&mut self) {
        if self.last_emit.elapsed() >= EMIT_EVERY {
            self.emit();
        }
    }

    /// Count `n` bytes towards the speed without moving the progress (a sync
    /// measures the wire, but advances per finished file).
    pub(crate) fn sample_speed(&mut self, n: u64) {
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

/// The job's reporter, shared by the files that copy at the same time.
type SharedReporter = Mutex<Reporter>;

enum Decision {
    Skip,
    /// Copy; `replaces` means an existing destination gets backed up first.
    Copy {
        replaces: bool,
    },
}

/// One running job: what its files share while they copy.
struct Job<'a> {
    session: &'a Session,
    direction: Direction,
    policy: Conflict,
    ctl: &'a Ctl,
    rep: &'a SharedReporter,
    /// Collects what the job overwrites.
    undo: &'a Recorder,
}

impl Job<'_> {
    async fn decide(&self, item: &Item, listings: &Listings) -> AppResult<Decision> {
        let session = self.session;
        let dst_mtime = match self.direction {
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
        let copy = match self.policy {
            Conflict::Overwrite => true,
            Conflict::Skip => false,
            // Remote times can be coarse (FTP LIST: minutes), so allow that much slack.
            Conflict::Newer => dst_mtime.is_none_or(|d| item.mtime > d + tolerance),
        };
        Ok(if copy { Decision::Copy { replaces: true } } else { Decision::Skip })
    }

    /// Copy `item` to `dst`: its destination, or what a symlink there points to.
    async fn copy_file(&self, item: &Item, dst: &str, kept: Option<&Kept>) -> AppResult<()> {
        let fs = self.session.fs()?;
        let progress = |n: usize| {
            self.rep.lock().unwrap().add_bytes(n as u64);
            self.ctl.checkpoint()
        };
        match self.direction {
            Direction::Upload => {
                let mut src = tokio::fs::File::open(&item.src).await?;
                let mut out = fs.writer(dst).await?;
                copy_chunks(&mut src, &mut out, progress).await?;
                out.finish().await?;
                // Keep the source's modification time, so "only newer" works both ways.
                fs.set_mtime(dst, item.mtime).await;
                if let Some(kept) = kept {
                    fs.restore_kept(dst, kept).await;
                }
            }
            Direction::Download => {
                let mut src = fs.reader(&item.src).await?;
                let mut dst = tokio::fs::File::create(&item.dst).await?;
                copy_chunks(&mut src, &mut dst, progress).await?;
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
    async fn make_dirs(&self, items: &[Item]) -> AppResult<Listings> {
        let mut listings = Listings::default();
        for item in items.iter().filter(|i| i.is_dir) {
            self.ctl.checkpoint().await?;
            match self.direction {
                Direction::Upload => {
                    let fs = self.session.fs()?;
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

    async fn transfer_file(&self, item: &Item, listings: &Listings) -> AppResult<()> {
        let (session, rep, undo) = (self.session, self.rep, self.undo);
        self.ctl.checkpoint().await?;
        let side = match self.direction {
            Direction::Upload => Some(session),
            Direction::Download => None,
        };
        let mut dst = item.dst.clone();
        let mut keep = None;
        let replaces = match self.decide(item, listings).await? {
            Decision::Skip => {
                let mut rep = rep.lock().unwrap();
                rep.progress.skipped += 1;
                rep.progress.bytes_total -= item.size;
                return Ok(());
            }
            Decision::Copy { replaces } => replaces,
        };
        if replaces {
            // The listing already says what the destination is; no lock either, so
            // overwrites run as parallel as the copies.
            // A symlink is followed: the file it points to is backed up and rewritten, and keeps its mode.
            let (target, kept) = match self.direction {
                Direction::Upload => session.fs()?.overwrite_target(&item.dst).await?,
                Direction::Download => (item.dst.clone(), None),
            };
            dst = target;
            keep = kept;
            let is_dir = if dst == item.dst { listings.entries.get(&item.dst).map(|m| m.is_dir && !m.is_symlink) } else { None };
            undo.stash_known(side, &dst, is_dir).await?;
            rep.lock().unwrap().progress.backup_id = Some(undo.id().to_string());
        }
        if let Err(e) = self.copy_file(item, &dst, keep.as_ref()).await {
            // Don't leave a half-written file behind.
            match self.direction {
                Direction::Upload => {
                    let _ = session.fs()?.remove_file(&dst).await;
                }
                Direction::Download => {
                    let _ = std::fs::remove_file(&dst);
                }
            }
            // And put back the file it was replacing.
            if replaces {
                match undo.put_back(side, &dst).await {
                    Ok(()) if undo.is_empty() => rep.lock().unwrap().progress.backup_id = None,
                    Ok(()) => {}
                    Err(err) => eprintln!("kade: restoring {dst} failed, it stays in the backup: {err}"),
                }
            }
            return Err(e);
        }
        let mut rep = rep.lock().unwrap();
        rep.progress.files_done += 1;
        rep.emit();
        Ok(())
    }

    async fn run(&self, source: &str) -> AppResult<()> {
        let (session, direction) = (self.session, self.direction);
        // A job cancelled or paused while queued must not start scanning.
        self.ctl.checkpoint().await?;
        let dest = {
            let mut rep = self.rep.lock().unwrap();
            rep.state(JobState::Scanning);
            rep.progress.dest.clone()
        };
        let items = match direction {
            // A big local tree (node_modules) takes a while to walk: not on an async worker.
            Direction::Upload => {
                let (source, dest) = (source.to_string(), dest.clone());
                tauri::async_runtime::spawn_blocking(move || scan_local(&source, &dest)).await.map_err(AppError::other)??
            }
            Direction::Download => scan_remote(session, self.ctl, source, Path::new(&dest)).await?,
        };
        {
            let mut rep = self.rep.lock().unwrap();
            rep.progress.files_total = items.iter().filter(|i| !i.is_dir).count() as u64;
            rep.progress.bytes_total = items.iter().map(|i| i.size).sum();
            rep.state(JobState::Running);
        }

        // Scanning pushes parents before children, so this creates them in order.
        let listings = self.make_dirs(&items).await?;

        // Small files are dominated by round trips, so copy several at once.
        let parallel = session.fs()?.parallel_files();
        let mut files = items.iter().filter(|i| !i.is_dir);
        let mut running = FuturesUnordered::new();
        let mut failure = None;
        loop {
            while failure.is_none() && running.len() < parallel {
                let Some(item) = files.next() else { break };
                running.push(self.transfer_file(item, &listings));
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
}

/// What the user asked to copy: each source into `dest_dir`.
pub struct JobSpec {
    pub direction: Direction,
    pub sources: Vec<String>,
    pub dest_dir: String,
    pub policy: Conflict,
}

/// Queue one job per source. Returns the job ids immediately.
pub fn start(emit: Emit, transfers: Arc<Transfers>, session: Arc<Session>, session_id: String, spec: JobSpec) -> Vec<String> {
    let JobSpec { direction, sources, dest_dir, policy } = spec;
    let mut ids = Vec::new();
    for source in sources {
        let id = uuid::Uuid::new_v4().to_string();
        let name = basename(&source);
        let dest = match direction {
            Direction::Upload => join_remote(&dest_dir, &name),
            Direction::Download => Path::new(&dest_dir).join(&name).to_string_lossy().into_owned(),
        };
        let ctl = transfers.register(&id, true);

        let mut rep = Reporter::new(
            emit.clone(),
            Progress {
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
                kind: JobKind::Transfer,
                warning: None,
            },
        );
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
            transfers.finish(&rep.progress.id);
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
    let undo = Recorder::new(
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
    let result = Job { session, direction, policy, ctl, rep, undo: &undo }.run(source).await;
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
                kind: JobKind::Transfer,
                warning: None,
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
        // Biased, watcher first: it sees each change before the job can finish.
        let result = tokio::select! {
            biased;
            never = ctl.report_pauses(&rep) => match never {},
            r = job => r,
        };
        assert!(result.is_ok());
        let states = states.lock().unwrap().clone();
        assert!(states.ends_with(&[JobState::Paused, JobState::Running]), "{states:?}");
    }

    #[test]
    fn sync_jobs_ignore_pause() {
        let transfers = Transfers::default();
        let sync = transfers.register("s", false);
        let copy = transfers.register("t", true);
        transfers.pause("s", true);
        transfers.pause("t", true);
        assert!(!sync.flags.borrow().paused && copy.flags.borrow().paused);
        transfers.cancel("s");
        assert!(sync.cancelled());
        transfers.finish("s");
        assert!(transfers.ctl("s").is_none());
    }
}
