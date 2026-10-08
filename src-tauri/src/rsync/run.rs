//! Running rsync: the process with its relay and bridge, and the sync job in
//! the transfer queue (progress, cancel, backups, deletes).

use std::convert::Infallible;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;

use super::args::{self, Pass};
use super::bridge::{self, RemoteSpec};
use super::detect::Detected;
use super::itemize::{self, Kind};
use super::{relay, Direction, Planned, RunDir, SyncRequest};
use crate::backup::{self, BackupEntry, Op, Recorder, Side};
use crate::error::{AppError, AppResult};
use crate::ssh::{join_remote, Session};
use crate::transfer::{self, Ctl, Emit, JobKind, JobState, Progress, Reporter, Transfers};

/// After SIGTERM rsync gets this long to remove its temp file before SIGKILL.
const TERM_GRACE: Duration = Duration::from_secs(5);
const STDERR_TAIL: usize = 20;

/// One rsync process.
pub struct Invocation<'a> {
    pub session: &'a Session,
    pub detected: &'a Detected,
    pub request: &'a SyncRequest,
    pub pass: Pass,
    /// `--backup-dir` on the destination side, for [`Pass::Run`].
    pub backup_dir: Option<String>,
    pub excludes: &'a [String],
}

pub struct Outcome {
    /// `None` when rsync died from a signal.
    pub code: Option<i32>,
    /// The last lines rsync (and the server) wrote to stderr.
    pub stderr: String,
}

// glibc before 2.34 keeps openpty in libutil.
#[cfg(target_os = "linux")]
#[link(name = "util")]
extern "C" {}

/// A pseudo-terminal for rsync's stdout. openrsync's stdio buffers a pipe in
/// 4 KiB blocks, so its lines (progress, and the streamed backup journal)
/// would only arrive when it exits; on a terminal it writes each line.
fn line_buffered_stdout() -> std::io::Result<(std::fs::File, OwnedFd)> {
    let (mut master, mut slave) = (0, 0);
    // SAFETY: out-pointers to two ints; the optional arguments are null.
    if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openpty just returned these descriptors, and nothing else owns them.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    // SAFETY: plain fcntl/termios calls on descriptors we own.
    unsafe {
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        // Raw: no "\n" → "\r\n", no echo, bytes passed as they are.
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(slave.as_raw_fd(), &mut t) == 0 {
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &t);
        }
    }
    Ok((std::fs::File::from(master), slave))
}

fn signal(pid: Option<u32>, sig: i32) {
    if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
        // SAFETY: kill has no memory effects; a stale pid at worst gets ESRCH.
        unsafe { libc::kill(pid, sig) };
    }
}

/// Run rsync once, with the relay on the session. `on_line` gets each stdout
/// line, `on_wire` each count of bytes over the SSH channel. Cancelling sends
/// SIGTERM (so rsync removes its temp file), then SIGKILL.
pub async fn rsync(
    inv: Invocation<'_>,
    cancel: &CancellationToken,
    mut on_line: impl FnMut(&[u8]),
    on_wire: &(dyn Fn(u64) + Sync),
) -> AppResult<Outcome> {
    let local = inv.detected.local.as_ref().ok_or_else(|| AppError::other(tr!("rsync isn't available", "rsync is niet beschikbaar")))?;
    let remote = RemoteSpec {
        rsync: inv.detected.remote_path.clone().unwrap_or_else(|| "rsync".into()),
        path: inv.request.remote.clone(),
        direction: inv.request.direction,
        backup_dir: if inv.request.direction == Direction::ToServer { inv.backup_dir.clone() } else { None },
    };
    let run = RunDir::create()?;
    run.write_rsh()?;
    run.write_excludes(inv.excludes)?;
    let token = (0..3).map(|_| uuid::Uuid::new_v4().simple().to_string()).collect::<String>();
    let sock = run.sock();
    let listener = UnixListener::bind(&sock)?;

    let mut cmd = crate::hostenv::async_command(&local.path);
    cmd.args(args::build(inv.request, inv.pass, inv.backup_dir.as_deref(), &run.rsh(), &run.excludes()))
        .env(relay::TOKEN_VAR, &token)
        .env(relay::SOCK_VAR, &sock)
        // A user's environment must not route rsync around the relay.
        .env_remove("RSYNC_RSH")
        .env_remove("RSYNC_CONNECT_PROG")
        .env_remove("RSYNC_PROXY")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let (stdout, tty) = line_buffered_stdout()?;
    cmd.stdout(Stdio::from(tty));
    let spawned = cmd.spawn();
    // Our copy of the terminal's other end must go, or reading never sees the end.
    drop(cmd);
    let mut child = spawned.map_err(|e| AppError::other(tr!("Can't start rsync: {e}", "Kan rsync niet starten: {e}", e = e)))?;
    let pid = child.id();
    let stdout = tokio::fs::File::from_std(stdout);
    let stderr = child.stderr.take().expect("piped");

    let bridge = async {
        let conn = bridge::accept(listener, &sock, &token, &remote).await?;
        bridge::pump(conn, inv.session, on_wire).await
    };
    let driver = async {
        tokio::pin!(bridge);
        let mut bridged: Option<AppResult<()>> = None;
        let mut kill_at: Option<tokio::time::Instant> = None;
        let status = loop {
            let deadline = kill_at.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(86400));
            tokio::select! {
                r = &mut bridge, if bridged.is_none() => {
                    // The relay was refused or failed: rsync can't get anywhere.
                    if r.is_err() {
                        signal(pid, libc::SIGKILL);
                    }
                    bridged = Some(r);
                }
                s = child.wait() => break s,
                () = cancel.cancelled(), if kill_at.is_none() => {
                    signal(pid, libc::SIGTERM);
                    kill_at = Some(tokio::time::Instant::now() + TERM_GRACE);
                }
                () = tokio::time::sleep_until(deadline), if kill_at.is_some() => signal(pid, libc::SIGKILL),
            }
        };
        // Let the channel close cleanly, so the server's rsync sees EOF and tidies up.
        if bridged.is_none() {
            bridged = tokio::time::timeout(TERM_GRACE, &mut bridge).await.ok();
        }
        (status, bridged)
    };
    let lines = async {
        let mut out = BufReader::new(stdout).split(b'\n');
        // A terminal reports its end as EIO on Linux, as EOF on macOS.
        while let Ok(Some(line)) = out.next_segment().await {
            on_line(&line);
        }
    };
    let errors = async {
        let mut tail = std::collections::VecDeque::new();
        let mut err = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = err.next_line().await {
            if tail.len() == STDERR_TAIL {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        Vec::from(tail).join("\n")
    };
    let ((status, bridged), (), stderr) = tokio::join!(driver, lines, errors);
    let status = status?;
    if let Some(Err(e)) = bridged {
        if !cancel.is_cancelled() {
            if !stderr.is_empty() {
                eprintln!("kade: rsync: {stderr}");
            }
            return Err(e);
        }
    }
    Ok(Outcome { code: status.code(), stderr })
}

fn source_changed() -> AppError {
    AppError::other(tr!(
        "The source changed since the preview; preview again",
        "De bron is veranderd sinds het voorbeeld; bekijk het voorbeeld opnieuw"
    ))
}

/// The error for an rsync exit code; `None` when the run counts as done
/// (0, or 24: source files vanished during the run).
pub fn exit_error(code: Option<i32>, stderr: &str) -> Option<AppError> {
    let tail = stderr.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    let text = match code {
        Some(0 | 24) => return None,
        None => tr!("rsync was stopped", "rsync is gestopt"),
        Some(25) => return Some(source_changed()),
        Some(30 | 35) => tr!("The server stopped responding", "De server reageert niet meer"),
        Some(12 | 255) => {
            tr!("The connection to the server was lost. {tail}", "De verbinding met de server is verbroken. {tail}", tail = tail)
        }
        Some(23) => {
            tr!("Some files couldn't be synced: {tail}", "Sommige bestanden konden niet worden gesynchroniseerd: {tail}", tail = tail)
        }
        Some(code) => tr!("rsync failed (exit {code}): {tail}", "rsync mislukt (code {code}): {tail}", code = code, tail = tail),
    };
    Some(AppError::other(text.trim_end()))
}

fn cancelled() -> AppError {
    AppError::other(tr!("cancelled", "geannuleerd"))
}

/// Queue a previewed sync as a job. Returns the job id immediately; progress
/// comes through `emit` like a transfer's, with `kind: "sync"`.
pub fn start(emit: Emit, transfers: Arc<Transfers>, session: Arc<Session>, detected: Arc<Detected>, plan: Planned) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let ctl = transfers.register(&id, false);
    let req = &plan.request;
    let mut rep = Reporter::new(
        emit,
        Progress {
            id: id.clone(),
            session_id: plan.session_id.clone(),
            name: transfer::basename(req.source()),
            direction: match req.direction {
                Direction::ToServer => transfer::Direction::Upload,
                Direction::FromServer => transfer::Direction::Download,
            },
            dest: req.dest().to_string(),
            state: JobState::Queued,
            bytes_done: 0,
            bytes_total: plan.bytes_total,
            files_done: 0,
            files_total: plan.files_total,
            skipped: 0,
            speed: 0.0,
            error: None,
            backup_id: None,
            kind: JobKind::Sync,
            warning: None,
        },
    );
    rep.emit();
    tauri::async_runtime::spawn(async move {
        let rep = Mutex::new(rep);
        let result = run_job(&transfers, &session, &detected, &plan, &ctl, &rep).await;
        let mut rep = rep.into_inner().unwrap();
        rep.progress.speed = 0.0;
        match result {
            Ok(warning) => {
                rep.progress.warning = warning;
                rep.state(JobState::Done);
            }
            Err(_) if ctl.cancelled() => rep.state(JobState::Cancelled),
            Err(e) => {
                rep.progress.error = Some(e.to_string());
                rep.state(JobState::Failed);
            }
        }
        transfers.finish(&rep.progress.id);
    });
    id
}

async fn run_job(
    transfers: &Transfers,
    session: &Session,
    detected: &Detected,
    plan: &Planned,
    ctl: &Ctl,
    rep: &Mutex<Reporter>,
) -> AppResult<Option<String>> {
    let _slot = transfers.slots.acquire().await;
    // Cancelled while queued.
    ctl.checkpoint().await?;
    rep.lock().unwrap().state(JobState::Running);
    let req = &plan.request;
    let side = match req.direction {
        Direction::ToServer => Side::Remote,
        Direction::FromServer => Side::Local,
    };
    let remote_side = (side == Side::Remote).then_some(session);
    let dest_root = req.dest().trim_end_matches('/').to_string();
    let undo =
        Recorder::new(side, Op::Overwrite, remote_side, tr!("Sync of {} to {}", "Synchronisatie van {} naar {}", req.source(), req.dest()));
    let cancel = CancellationToken::new();
    let watch_cancel = async {
        ctl.until_cancelled().await;
        cancel.cancel();
        std::future::pending::<Infallible>().await
    };
    let job = async {
        // No --mkpath everywhere: the destination is created first.
        match req.direction {
            Direction::ToServer => session.fs()?.mkdir_p(&dest_root).await?,
            Direction::FromServer => std::fs::create_dir_all(&dest_root)?,
        }
        let backup_dir = undo.backup_dir(Some(session), &dest_root)?;
        // Progress follows the bytes on the SSH channel, not the itemize lines:
        // the sender prints a file's line when it starts, so lines run ahead.
        let (started, wire) = (AtomicU64::new(0), AtomicU64::new(0));
        let on_line = |line: &[u8]| {
            let Some(item) = itemize::parse_line(line) else { return };
            let mut rep = rep.lock().unwrap();
            match item.kind {
                Kind::File { new } => {
                    let started = started.fetch_add(1, Relaxed) + 1;
                    // While the sender is still on the last line, that file isn't done.
                    rep.progress.files_done = match req.direction {
                        Direction::ToServer => started.saturating_sub(1),
                        Direction::FromServer => started,
                    };
                    if new {
                        undo.record(BackupEntry::created(join_remote(&dest_root, &item.name), false, side));
                    } else {
                        // Streamed, so a crash mid-run still leaves a restorable journal;
                        // reconcile below makes it exact.
                        undo.record(BackupEntry {
                            original: join_remote(&dest_root, &item.name),
                            stored: join_remote(&backup_dir, &item.name),
                            stored_on: side,
                            is_dir: false,
                            created: false,
                        });
                    }
                    rep.progress.backup_id = Some(undo.id().to_string());
                }
                // New folders and links are undone too; a folder covers what's in it.
                Kind::Dir { new: true } | Kind::Other { new: true } => {
                    undo.record(BackupEntry::created(join_remote(&dest_root, &item.name), item.is_dir, side));
                    rep.progress.backup_id = Some(undo.id().to_string());
                }
                _ => return,
            }
            rep.tick();
        };
        let on_wire = |n: u64| {
            let wire = wire.fetch_add(n, Relaxed) + n;
            let mut rep = rep.lock().unwrap();
            let total = rep.progress.bytes_total;
            // Protocol chatter counts too, so stay just under the total until rsync is done.
            rep.progress.bytes_done = if total > 0 { wire.min(total - total / 100) } else { 0 };
            rep.sample_speed(n);
        };
        let inv =
            Invocation { session, detected, request: req, pass: Pass::Run, backup_dir: Some(backup_dir.clone()), excludes: &plan.excludes };
        let outcome = rsync(inv, &cancel, on_line, &on_wire).await;
        // The backup folder is the truth, whatever happened (session gone: the streamed entries stay).
        if let Err(e) = undo.reconcile(remote_side, &backup_dir, &dest_root).await {
            eprintln!("kade: checking the sync's backup folder failed: {e}");
        }
        rep.lock().unwrap().progress.backup_id = (!undo.is_empty()).then(|| undo.id().to_string());
        let outcome = outcome?;
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        if let Some(e) = exit_error(outcome.code, &outcome.stderr) {
            return Err(e);
        }
        {
            let mut rep = rep.lock().unwrap();
            rep.progress.bytes_done = plan.bytes_total;
            rep.progress.files_done = started.load(Relaxed);
        }
        let warning = (outcome.code == Some(24))
            .then(|| tr!("Some source files disappeared during the sync", "Sommige bronbestanden verdwenen tijdens het synchroniseren"));
        if req.delete && !plan.deletes.is_empty() {
            delete_pass(session, detected, plan, &undo, &dest_root, &cancel).await?;
            rep.lock().unwrap().progress.backup_id = (!undo.is_empty()).then(|| undo.id().to_string());
        }
        AppResult::Ok(warning)
    };
    let result = tokio::select! {
        r = job => r,
        never = watch_cancel => match never {},
    };
    // Recorded even after a failure: what was replaced so far is in it.
    let tx = undo.commit().unwrap_or_else(|e| {
        eprintln!("kade: backup-index bijwerken mislukt: {e}");
        None
    });
    rep.lock().unwrap().progress.backup_id = tx.map(|t| t.id);
    result
}

/// Delete what the source no longer has, through the backup like any delete.
/// Only what the preview showed: a longer list means the source changed.
async fn delete_pass(
    session: &Session,
    detected: &Detected,
    plan: &Planned,
    undo: &Recorder,
    dest_root: &str,
    cancel: &CancellationToken,
) -> AppResult<()> {
    let mut found = Vec::new();
    let inv = Invocation { session, detected, request: &plan.request, pass: Pass::Deletions, backup_dir: None, excludes: &plan.excludes };
    let outcome = rsync(
        inv,
        cancel,
        |line| {
            if let Some(item) = itemize::parse_line(line).filter(|i| i.kind == Kind::Deleted) {
                found.push(item);
            }
        },
        &|_| {},
    )
    .await?;
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    if let Some(e) = exit_error(outcome.code, &outcome.stderr) {
        return Err(e);
    }
    if found.iter().any(|i| !plan.deletes.contains(&i.name)) {
        return Err(source_changed());
    }
    // rsync lists a folder's contents before the folder; moving the folder moves them too.
    let dirs: Vec<&str> = found.iter().filter(|i| i.is_dir).map(|i| i.name.as_str()).collect();
    let session = (undo.side() == Side::Remote).then_some(session);
    for item in &found {
        if dirs.iter().any(|d| *d != item.name && backup::is_within(&item.name, d)) {
            continue;
        }
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        undo.stash_known(session, &join_remote(dest_root, &item.name), Some(item.is_dir)).await?;
    }
    Ok(())
}
