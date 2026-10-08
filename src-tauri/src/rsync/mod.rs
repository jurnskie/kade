//! "Sync folder": a one-way sync with the local `rsync` binary, its traffic
//! carried over Kade's own SSH session (so every sign-in route keeps working).
//!
//! rsync is given `-e <rundir>/rsh`, a script that starts this same binary as
//! `kade --rsync-relay` ([`relay`]). The relay hands rsync's server command to
//! the app over a private socket ([`bridge`]), which validates it, rebuilds it
//! and runs it on an exec channel. A dry run ([`preview`]) shows what will
//! change; the real run ([`run`]) puts every replaced file in a backup
//! transaction through `--backup-dir`, and Kade does the deletes itself.

pub mod args;
pub mod bridge;
pub mod detect;
pub mod itemize;
pub mod preview;
pub mod relay;
pub mod run;

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::backup;
use crate::error::{AppError, AppResult};
use crate::ssh::Session;
use detect::Detected;

/// A preview that isn't started within this time must be made again.
const PREVIEW_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// From the local folder to the server folder.
    ToServer,
    FromServer,
}

/// What the user asked for in the sync dialog.
#[derive(Debug, Clone, Deserialize)]
pub struct SyncRequest {
    pub direction: Direction,
    /// Absolute local folder: the source, or the destination.
    pub local: String,
    /// Absolute server folder.
    pub remote: String,
    /// Delete files at the destination that aren't in the source (backed up).
    pub delete: bool,
    /// Compare contents instead of size and time (`-c`).
    pub checksum: bool,
    /// One pattern per entry, as typed.
    pub excludes: Vec<String>,
}

impl SyncRequest {
    fn dest(&self) -> &str {
        match self.direction {
            Direction::ToServer => &self.remote,
            Direction::FromServer => &self.local,
        }
    }

    fn source(&self) -> &str {
        match self.direction {
            Direction::ToServer => &self.local,
            Direction::FromServer => &self.remote,
        }
    }
}

/// A previewed sync, kept until it is started: the real run may only do what
/// the user saw (and the deletes are checked against it).
#[derive(Debug, Clone)]
pub struct Planned {
    pub session_id: String,
    pub request: SyncRequest,
    /// Every exclude line: the user's, Kade's backup folders, skipped symlinks.
    pub excludes: Vec<String>,
    /// Relative paths the preview listed as deleted.
    pub deletes: HashSet<String>,
    pub files_total: u64,
    pub bytes_total: u64,
    created: Instant,
}

/// App state: detection per session, and previews waiting to be started.
#[derive(Default)]
pub struct Syncs {
    detected: Mutex<HashMap<String, Arc<Detected>>>,
    planned: Mutex<HashMap<String, Planned>>,
    previews: Mutex<HashMap<String, CancellationToken>>,
}

impl Syncs {
    /// Detection runs once per session, the first time it's asked for.
    pub async fn detected(&self, session_id: &str, session: &Session) -> Arc<Detected> {
        if let Some(d) = self.detected.lock().unwrap().get(session_id) {
            return d.clone();
        }
        let d = Arc::new(detect::detect(session).await);
        self.detected.lock().unwrap().insert(session_id.to_string(), d.clone());
        d
    }

    pub fn forget_session(&self, session_id: &str) {
        self.detected.lock().unwrap().remove(session_id);
        self.planned.lock().unwrap().retain(|_, p| p.session_id != session_id);
    }

    pub fn plan(&self, id: String, planned: Planned) {
        let mut all = self.planned.lock().unwrap();
        all.retain(|_, p| p.created.elapsed() < PREVIEW_TTL);
        all.insert(id, planned);
    }

    pub fn take_plan(&self, id: &str) -> AppResult<Planned> {
        self.planned
            .lock()
            .unwrap()
            .remove(id)
            .filter(|p| p.created.elapsed() < PREVIEW_TTL)
            .ok_or_else(|| AppError::other(tr!("This preview has expired; preview again", "Dit voorbeeld is verlopen; bekijk het opnieuw")))
    }

    /// A cancel handle for a running preview, by the id the frontend chose.
    pub fn preview_started(&self, id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        self.previews.lock().unwrap().insert(id.to_string(), token.clone());
        token
    }

    pub fn preview_finished(&self, id: &str) {
        self.previews.lock().unwrap().remove(id);
    }

    pub fn cancel_preview(&self, id: &str) {
        if let Some(token) = self.previews.lock().unwrap().get(id) {
            token.cancel();
        }
    }
}

fn plain_path(path: &str) -> bool {
    path.starts_with('/') && !path.contains(['\n', '\r', '\0'])
}

/// Refuse a sync that is malformed or would touch something it must not:
/// home or `/` as the destination, or Kade's own backup folders on either side.
pub(crate) async fn check_request(session: &Session, req: &SyncRequest) -> AppResult<()> {
    if !plain_path(&req.local) || !plain_path(&req.remote) {
        return Err(AppError::other(tr!(
            "Sync needs absolute folder paths without line breaks",
            "Synchroniseren heeft absolute mappaden zonder regeleinden nodig"
        )));
    }
    let refuse = |path: &str| Err(AppError::other(tr!("Refusing to sync {path}", "Weigert {path} te synchroniseren", path = path)));
    let local_backups = backup::local_root()?.to_string_lossy().into_owned();
    let local_dest = req.direction == Direction::FromServer;
    if backup::is_within(&req.local, &local_backups) || (local_dest && backup::is_within(&local_backups, &req.local)) {
        return refuse(&req.local);
    }
    if let Some(kade) = backup::remote_kade_dir(session) {
        if backup::is_within(&req.remote, &kade) {
            return refuse(&req.remote);
        }
    }
    match req.direction {
        Direction::ToServer => {
            backup::guard_remote(session, &req.remote)?;
            match std::fs::metadata(&req.local) {
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Err(AppError::other(tr!("{p} is not a folder", "{p} is geen map", p = req.local))),
                Err(_) => {
                    return Err(AppError::other(tr!(
                        "{p} doesn't exist on this computer. Change the “From” folder.",
                        "{p} bestaat niet op deze computer. Pas de map bij “Van” aan.",
                        p = req.local
                    )))
                }
            }
        }
        Direction::FromServer => {
            crate::fs::guard(&req.local)?;
            match session.fs()?.stat(&req.remote).await? {
                Some(m) if m.is_dir => {}
                Some(_) => return Err(AppError::other(tr!("{p} is not a folder", "{p} is geen map", p = req.remote))),
                None => {
                    return Err(AppError::other(tr!(
                        "{p} doesn't exist on the server. Change the “From” folder.",
                        "{p} bestaat niet op de server. Pas de map bij “Van” aan.",
                        p = req.remote
                    )))
                }
            }
        }
    }
    Ok(())
}

/// The relay binary: this one. Debug builds may point elsewhere, because
/// under `cargo test` this is the test harness, which has no relay mode.
fn relay_exe() -> AppResult<PathBuf> {
    if cfg!(debug_assertions) {
        if let Some(exe) = std::env::var_os("KADE_RSYNC_RELAY") {
            return Ok(PathBuf::from(exe));
        }
    }
    Ok(std::env::current_exe()?)
}

/// Unix socket paths are limited (104 bytes on macOS, 108 on Linux).
const SUN_PATH_MAX: usize = 100;

/// A private folder for one rsync run: the `rsh` script, the socket and the
/// exclude file. Removed when dropped.
pub(crate) struct RunDir {
    dir: PathBuf,
}

impl RunDir {
    pub(crate) fn create() -> AppResult<RunDir> {
        let name = format!("kade-rsync-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let mut dir = std::env::temp_dir().join(&name);
        // rsync splits `-e` on whitespace, and the socket path must be short.
        let usable = |d: &Path| {
            let s = d.join("sock").to_string_lossy().into_owned();
            s.len() <= SUN_PATH_MAX && !s.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\')
        };
        if !usable(&dir) {
            // SAFETY: getuid has no preconditions and can't fail.
            let base = PathBuf::from(format!("/tmp/kade-{}", unsafe { libc::getuid() }));
            private_dir(&base)?;
            dir = base.join(name);
        }
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        Ok(RunDir { dir })
    }

    pub(crate) fn sock(&self) -> PathBuf {
        self.dir.join("sock")
    }

    pub(crate) fn rsh(&self) -> PathBuf {
        self.dir.join("rsh")
    }

    pub(crate) fn excludes(&self) -> PathBuf {
        self.dir.join("excludes")
    }

    /// The script rsync runs as its remote shell. Fixed arguments, and the
    /// AppImage's library path back (hostenv strips it from rsync).
    pub(crate) fn write_rsh(&self) -> AppResult<()> {
        let exe = relay_exe()?;
        let quote = |s: &str| {
            shlex::try_quote(s)
                .map(|q| q.into_owned())
                .map_err(|_| AppError::other(tr!("Can't use {s} for rsync", "Kan {s} niet voor rsync gebruiken", s = s)))
        };
        let mut env = String::new();
        if std::env::var_os("APPIMAGE").is_some() {
            if let Ok(lib) = std::env::var("LD_LIBRARY_PATH") {
                env = format!("env LD_LIBRARY_PATH={} ", quote(&lib)?);
            }
        }
        let script = format!("#!/bin/sh\nexec {env}{} --rsync-relay \"$@\"\n", quote(&exe.to_string_lossy())?);
        std::fs::write(self.rsh(), script)?;
        std::fs::set_permissions(self.rsh(), std::fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    pub(crate) fn write_excludes(&self, lines: &[String]) -> AppResult<()> {
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(self.excludes(), text)?;
        Ok(())
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `/tmp/kade-<uid>`, created 0700, or refused when someone else made it.
fn private_dir(dir: &Path) -> AppResult<()> {
    if let Err(e) = std::fs::DirBuilder::new().mode(0o700).create(dir) {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(e.into());
        }
    }
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no preconditions and can't fail.
    let mine = meta.uid() == unsafe { libc::getuid() };
    if !meta.is_dir() || !mine || meta.mode() & 0o077 != 0 {
        return Err(AppError::other(tr!("{dir} is not a private folder", "{dir} is geen privémap", dir = dir.display())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_dir_is_private_and_removed() {
        let run = RunDir::create().unwrap();
        let dir = run.dir.clone();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(run.sock().to_string_lossy().len() <= SUN_PATH_MAX);
        run.write_rsh().unwrap();
        let script = std::fs::read_to_string(run.rsh()).unwrap();
        assert!(script.starts_with("#!/bin/sh\nexec ") && script.ends_with(" --rsync-relay \"$@\"\n"), "{script}");
        assert_eq!(std::fs::metadata(run.rsh()).unwrap().permissions().mode() & 0o777, 0o700);
        drop(run);
        assert!(!dir.exists());
    }
}
