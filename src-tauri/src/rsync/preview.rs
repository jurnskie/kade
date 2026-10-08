//! The dry run the user approves before a sync, plus the one check rsync
//! can't do for Kade: a symlink to a *file* at the destination would be
//! replaced by a regular file (Kade's own transfers write through it). Those
//! are excluded from the sync and listed as skipped.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::args::{self, Pass};
use super::detect::Detected;
use super::itemize::{self, Item, Kind};
use super::run::{self, Invocation};
use super::{check_request, Direction, Planned, SyncRequest};
use crate::backup;
use crate::error::{AppError, AppResult};
use crate::ssh::{join_remote, Session};

/// The list the dialog shows is capped; the counts are not.
const MAX_ITEMS: usize = 5000;
/// Sizes of deleted files are looked up one by one, for this many at most.
const MAX_SIZE_LOOKUPS: usize = 200;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    New,
    Updated,
    Deleted,
    /// A symlinked file at the destination: left alone.
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewItem {
    pub change: Change,
    /// Relative to the source/destination folder.
    pub path: String,
    /// `None` for a deletion whose size couldn't be looked up (rsync gives none).
    pub size: Option<u64>,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncPreview {
    /// Pass to `sync_start`.
    pub id: String,
    pub new_files: u64,
    pub new_bytes: u64,
    pub updated_files: u64,
    pub updated_bytes: u64,
    /// Files and folders.
    pub deleted: u64,
    pub skipped: u64,
    pub items: Vec<PreviewItem>,
    /// `items` was cut off at its cap.
    pub truncated: bool,
    pub nothing_to_do: bool,
}

/// Which new files land where a symlink is now. Only new ones: rsync sees a
/// link where a file should be as "not there", and lists the file as new.
async fn symlinked(session: &Session, req: &SyncRequest, items: &[Item]) -> AppResult<HashSet<String>> {
    let created: HashSet<&str> = items.iter().filter(|i| i.kind == Kind::Dir { new: true }).map(|i| i.name.as_str()).collect();
    let parent = |name: &str| name.rsplit_once('/').map_or("", |(p, _)| p).to_string();
    let candidates = items.iter().filter(|i| i.kind == Kind::File { new: true } && !created.contains(parent(&i.name).as_str()));
    let mut links = HashSet::new();
    match req.direction {
        Direction::FromServer => {
            for item in candidates {
                let path = Path::new(&req.local).join(&item.name);
                if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
                    links.insert(item.name.clone());
                }
            }
        }
        Direction::ToServer => {
            // One listing per folder instead of a request per file.
            let mut listed: HashMap<String, HashSet<String>> = HashMap::new();
            let fs = session.fs()?;
            for item in candidates {
                let dir = parent(&item.name);
                if !listed.contains_key(&dir) {
                    let path = if dir.is_empty() { req.remote.clone() } else { join_remote(&req.remote, &dir) };
                    let names = fs.list(&path).await.unwrap_or_default().into_iter().filter(|e| e.is_symlink).map(|e| e.name).collect();
                    listed.insert(dir.clone(), names);
                }
                let name = item.name.rsplit('/').next().unwrap_or(&item.name);
                if listed[&dir].contains(name) {
                    links.insert(item.name.clone());
                }
            }
        }
    }
    Ok(links)
}

/// The size of a file the sync would delete, from the destination.
async fn deleted_size(session: &Session, req: &SyncRequest, name: &str) -> Option<u64> {
    match req.direction {
        Direction::FromServer => {
            let meta = std::fs::symlink_metadata(Path::new(&req.local).join(name)).ok()?;
            meta.is_file().then_some(meta.len())
        }
        Direction::ToServer => {
            let meta = session.fs().ok()?.stat(&join_remote(&req.remote, name)).await.ok()??;
            (!meta.is_dir).then_some(meta.size)
        }
    }
}

/// Exclude lines every pass of this sync uses.
fn base_excludes(session: &Session, req: &SyncRequest) -> AppResult<Vec<String>> {
    let mut lines = args::validate_excludes(&req.excludes)?;
    let local = backup::local_root()?.to_string_lossy().into_owned();
    let remote = backup::remote_kade_dir(session);
    lines.extend(args::backup_root_excludes(&[(&req.local, Some(&local)), (&req.remote, remote.as_deref())]));
    Ok(lines)
}

/// Dry-run the sync and keep the result as a plan that `sync_start` can run.
pub async fn preview(
    session: &Session,
    session_id: &str,
    detected: &Detected,
    id: String,
    req: SyncRequest,
    cancel: &CancellationToken,
) -> AppResult<(SyncPreview, Planned)> {
    if !detected.support.available {
        return Err(AppError::Unsupported { message: detected.support.reason.clone().unwrap_or_default() });
    }
    check_request(session, &req).await?;
    let mut excludes = base_excludes(session, &req)?;
    let dest_exists = match req.direction {
        Direction::ToServer => session.fs()?.stat(&req.remote).await?.is_some(),
        Direction::FromServer => Path::new(&req.local).exists(),
    };

    let mut items = Vec::new();
    let inv = Invocation { session, detected, request: &req, pass: Pass::Preview, backup_dir: None, excludes: &excludes };
    let outcome = run::rsync(inv, cancel, |line| items.extend(itemize::parse_line(line)), &|_| {}).await?;
    if cancel.is_cancelled() {
        return Err(AppError::other(tr!("cancelled", "geannuleerd")));
    }
    if let Some(e) = run::exit_error(outcome.code, &outcome.stderr) {
        return Err(e);
    }
    let links = if dest_exists { symlinked(session, &req, &items).await? } else { HashSet::new() };

    let mut preview = SyncPreview {
        id: id.clone(),
        new_files: 0,
        new_bytes: 0,
        updated_files: 0,
        updated_bytes: 0,
        deleted: 0,
        skipped: 0,
        items: Vec::new(),
        truncated: false,
        nothing_to_do: false,
    };
    let (mut files_total, mut bytes_total) = (0, 0);
    let mut deletes = HashSet::new();
    let mut lookups = 0;
    for item in items {
        let change = match item.kind {
            _ if links.contains(&item.name) => Change::Skipped,
            Kind::Deleted => Change::Deleted,
            // Without a destination folder rsync can't tell new from changed.
            Kind::File { new } | Kind::Other { new } if new || !dest_exists => Change::New,
            Kind::File { .. } | Kind::Other { .. } => Change::Updated,
            Kind::Dir { .. } | Kind::Unchanged => continue,
        };
        match change {
            Change::New => {
                preview.new_files += 1;
                preview.new_bytes += item.size;
            }
            Change::Updated => {
                preview.updated_files += 1;
                preview.updated_bytes += item.size;
            }
            Change::Deleted => {
                preview.deleted += 1;
                deletes.insert(item.name.clone());
            }
            Change::Skipped => {
                preview.skipped += 1;
                if item.name.contains(['\n', '\r']) {
                    return Err(AppError::other(tr!(
                        "Can't skip {name}: its name has a line break",
                        "Kan {name} niet overslaan: de naam bevat een regeleinde",
                        name = item.name
                    )));
                }
                excludes.push(args::anchored(&item.name, false));
            }
        }
        if matches!(item.kind, Kind::File { .. }) && matches!(change, Change::New | Change::Updated) {
            files_total += 1;
            bytes_total += item.size;
        }
        if preview.items.len() < MAX_ITEMS {
            let size = if change == Change::Deleted {
                if item.is_dir || lookups >= MAX_SIZE_LOOKUPS {
                    None
                } else {
                    lookups += 1;
                    deleted_size(session, &req, &item.name).await
                }
            } else {
                Some(item.size)
            };
            preview.items.push(PreviewItem { change, path: item.name, size, is_dir: item.is_dir });
        } else {
            preview.truncated = true;
        }
    }
    preview.nothing_to_do = preview.new_files + preview.updated_files + preview.deleted == 0;
    let planned =
        Planned { session_id: session_id.to_string(), request: req, excludes, deletes, files_total, bytes_total, created: Instant::now() };
    Ok((preview, planned))
}
