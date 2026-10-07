//! Temporary backups of everything Kade deletes or overwrites.
//!
//! Every destructive action is one *transaction*. Before a file or folder is
//! deleted or overwritten it is moved aside, so the transaction can be undone:
//! - on the server into `~/.cache/kade/backups/<tx>/<original path>` (a rename,
//!   so instant even for big folders, and outside any web root);
//! - locally into `<data dir>/kade/backups/<tx>/<original path>`.
//!
//! When a server-side rename is impossible (different filesystem), the item is
//! downloaded to the local backup folder instead. Transactions are listed in a
//! per-machine `index.json` and purged after the retention period.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::ssh::{join_remote, Session};
use crate::store::now_ms;

const REMOTE_ROOT: &str = ".cache/kade/backups";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Delete,
    Overwrite,
    /// Items that a restore replaced, so the restore itself can be undone.
    Restore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    pub original: String,
    pub stored: String,
    /// Where `stored` lives; can be Local for a remote item (download fallback).
    pub stored_on: Side,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: String,
    pub created: i64,
    pub side: Side,
    pub op: Op,
    pub summary: String,
    pub server_id: Option<String>,
    pub server_name: Option<String>,
    pub entries: Vec<BackupEntry>,
    #[serde(default)]
    pub restored: bool,
}

static INDEX: Mutex<()> = Mutex::new(());

fn local_root() -> AppResult<PathBuf> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| AppError::other(tr!("No data directory found", "Geen data-map gevonden")))?
        .join("kade/backups");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn load_index() -> AppResult<Vec<Transaction>> {
    let path = local_root()?.join("index.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn save_index(txs: &[Transaction]) -> AppResult<()> {
    let path = local_root()?.join("index.json");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(txs)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn list() -> AppResult<Vec<Transaction>> {
    let _guard = INDEX.lock().unwrap();
    let mut txs = load_index()?;
    txs.sort_by_key(|t| std::cmp::Reverse(t.created));
    Ok(txs)
}

fn remote_tx_root(session: &Session, id: &str) -> AppResult<String> {
    let home = session
        .remote_home
        .as_deref()
        .ok_or_else(|| AppError::other(tr!("Home directory on the server unknown", "Home-map op de server onbekend")))?;
    Ok(join_remote(&join_remote(home, REMOTE_ROOT), id))
}

/// True for paths inside Kade's own backup folder on the server; those are
/// deleted for real instead of being backed up again.
pub fn is_remote_backup_path(session: &Session, path: &str) -> bool {
    session.remote_home.as_deref().is_some_and(|home| path.starts_with(&join_remote(home, REMOTE_ROOT)))
}

/// Mirror an absolute path under a root: `/var/www/x` → `<root>/var/www/x`.
fn mirrored(root: &str, original: &str) -> String {
    join_remote(root, original.trim_start_matches('/'))
}

pub fn parent(path: &str) -> &str {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => "/",
        Some(i) => &path[..i],
    }
}

/// Copy a local file or tree (used when a rename crosses filesystems).
fn copy_local_tree(src: &Path, dst: &Path) -> AppResult<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_local_tree(&entry.path(), &dst.join(entry.file_name()))?;
        }
    } else if meta.file_type().is_symlink() {
        let target = std::fs::read_link(src)?;
        std::os::unix::fs::symlink(target, dst)?;
    } else {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

fn move_local(src: &Path, dst: &Path) -> AppResult<()> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    if std::fs::rename(src, dst).is_err() {
        copy_local_tree(src, dst)?;
        crate::fs::delete(&src.to_string_lossy())?;
    }
    Ok(())
}

async fn download_tree(session: &Session, src: &str, dst: &Path) -> AppResult<()> {
    let fs = session.fs()?;
    let mut stack = vec![(src.to_string(), dst.to_path_buf())];
    while let Some((src, dst)) = stack.pop() {
        if fs.is_real_dir(&src).await? {
            std::fs::create_dir_all(&dst)?;
            for entry in fs.list(&src).await? {
                stack.push((entry.path, dst.join(&entry.name)));
            }
        } else {
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut from = fs.reader(&src).await?;
            let mut to = tokio::fs::File::create(&dst).await?;
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                let n = from.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                tokio::io::AsyncWriteExt::write_all(&mut to, &buf[..n]).await?;
            }
            from.finish().await?;
        }
    }
    Ok(())
}

async fn upload_tree(session: &Session, src: &Path, dst: &str) -> AppResult<()> {
    let fs = session.fs()?;
    let mut stack = vec![(src.to_path_buf(), dst.to_string())];
    while let Some((src, dst)) = stack.pop() {
        if std::fs::metadata(&src)?.is_dir() {
            if !fs.exists(&dst).await? {
                fs.mkdir(&dst).await?;
            }
            for entry in std::fs::read_dir(&src)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                stack.push((entry.path(), join_remote(&dst, &name)));
            }
        } else {
            let mut from = tokio::fs::File::open(&src).await?;
            let mut to = fs.writer(&dst).await?;
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                let n = tokio::io::AsyncReadExt::read(&mut from, &mut buf).await?;
                if n == 0 {
                    break;
                }
                to.write_all(&buf[..n]).await?;
            }
            to.finish().await?;
        }
    }
    Ok(())
}

/// Collects the items of one transaction as they are moved aside.
pub struct Recorder {
    tx: Transaction,
    /// Remote backup folders known to exist, so each file's stash doesn't re-check the whole path.
    remote_dirs: HashSet<String>,
}

impl Recorder {
    pub fn new(side: Side, op: Op, session: Option<&Session>, summary: impl Into<String>) -> Self {
        Recorder {
            tx: Transaction {
                id: format!("{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), &uuid::Uuid::new_v4().to_string()[..8]),
                created: now_ms(),
                side,
                op,
                summary: summary.into(),
                server_id: session.map(|s| s.server_id.clone()),
                server_name: session.map(|s| s.server_name.clone()),
                entries: Vec::new(),
                restored: false,
            },
            remote_dirs: HashSet::new(),
        }
    }

    pub fn id(&self) -> &str {
        &self.tx.id
    }

    /// Move `path` into the backup. Afterwards `path` no longer exists.
    pub async fn stash(&mut self, session: Option<&Session>, path: &str) -> AppResult<()> {
        crate::fs::guard(path)?;
        let local_tx = local_root()?.join(&self.tx.id);
        let entry = match (self.tx.side, session) {
            (Side::Local, _) => {
                let src = Path::new(path);
                let is_dir = std::fs::symlink_metadata(src)?.is_dir();
                let dst = local_tx.join(path.trim_start_matches('/'));
                move_local(src, &dst)?;
                BackupEntry { original: path.into(), stored: dst.to_string_lossy().into(), stored_on: Side::Local, is_dir }
            }
            (Side::Remote, Some(session)) => {
                let fs = session.fs()?;
                let is_dir = fs.is_real_dir(path).await?;
                let local_copy = |dst: &Path| BackupEntry {
                    original: path.into(),
                    stored: dst.to_string_lossy().into(),
                    stored_on: Side::Local,
                    is_dir,
                };
                if fs.keeps_backups_on_server() {
                    let root = remote_tx_root(session, &self.tx.id)?;
                    let stored = mirrored(&root, path);
                    fs.mkdir_p_cached(parent(&stored), &mut self.remote_dirs).await?;
                    if fs.rename(path, &stored).await.is_ok() {
                        BackupEntry { original: path.into(), stored, stored_on: Side::Remote, is_dir }
                    } else {
                        // Different filesystem on the server: keep a local copy instead,
                        // and drop the now-unused folders we just created for it.
                        fs.rmdir_up(parent(&stored), &root).await;
                        self.remote_dirs.clear();
                        let dst = local_tx.join(path.trim_start_matches('/'));
                        download_tree(session, path, &dst).await?;
                        fs.delete_tree(path).await?;
                        local_copy(&dst)
                    }
                } else {
                    let dst = local_tx.join(path.trim_start_matches('/'));
                    download_tree(session, path, &dst).await?;
                    fs.delete_tree(path).await?;
                    local_copy(&dst)
                }
            }
            (Side::Remote, None) => return Err(AppError::SessionNotFound),
        };
        self.tx.entries.push(entry);
        Ok(())
    }

    /// Record the transaction (no-op when nothing was backed up).
    pub fn commit(self) -> AppResult<Option<Transaction>> {
        if self.tx.entries.is_empty() {
            return Ok(None);
        }
        let _guard = INDEX.lock().unwrap();
        let mut txs = load_index()?;
        txs.push(self.tx.clone());
        save_index(&txs)?;
        Ok(Some(self.tx))
    }
}

async fn exists(side: Side, session: Option<&Session>, path: &str) -> AppResult<bool> {
    Ok(match (side, session) {
        (Side::Local, _) => std::fs::symlink_metadata(path).is_ok(),
        (Side::Remote, Some(s)) => s.fs()?.exists(path).await?,
        (Side::Remote, None) => return Err(AppError::SessionNotFound),
    })
}

/// Put a transaction's items back. Whatever now occupies those paths is
/// itself backed up first, so a restore can be undone too.
pub async fn restore(id: &str, session: Option<&Session>) -> AppResult<Option<Transaction>> {
    let tx = list()?.into_iter().find(|t| t.id == id).ok_or_else(|| AppError::other(tr!("Backup not found", "Backup niet gevonden")))?;
    if tx.side == Side::Remote {
        let s = session.ok_or(AppError::SessionNotFound)?;
        if tx.server_id.as_deref() != Some(s.server_id.as_str()) {
            return Err(AppError::other(tr!("Connect to the server of this backup first", "Verbind eerst met de server van deze backup")));
        }
    }

    let mut undo = Recorder::new(tx.side, Op::Restore, session, tr!("Restoring: {}", "Terugzetten van: {}", tx.summary));
    for entry in &tx.entries {
        if exists(tx.side, session, &entry.original).await? {
            undo.stash(session, &entry.original).await?;
        }
        match (tx.side, entry.stored_on) {
            (Side::Local, _) => move_local(Path::new(&entry.stored), Path::new(&entry.original))?,
            (Side::Remote, Side::Remote) => {
                let s = session.ok_or(AppError::SessionNotFound)?;
                s.fs()?.mkdir_p(parent(&entry.original)).await?;
                s.fs()?.rename(&entry.stored, &entry.original).await?;
            }
            (Side::Remote, Side::Local) => {
                let s = session.ok_or(AppError::SessionNotFound)?;
                s.fs()?.mkdir_p(parent(&entry.original)).await?;
                upload_tree(s, Path::new(&entry.stored), &entry.original).await?;
                crate::fs::delete(&entry.stored)?;
            }
        }
    }

    {
        let _guard = INDEX.lock().unwrap();
        let mut txs = load_index()?;
        if let Some(t) = txs.iter_mut().find(|t| t.id == id) {
            t.restored = true;
        }
        save_index(&txs)?;
    }
    undo.commit()
}

/// Remove a transaction's stored data and its index entry.
async fn discard(tx: &Transaction, session: Option<&Session>) -> AppResult<()> {
    let local_tx = local_root()?.join(&tx.id);
    if local_tx.exists() {
        std::fs::remove_dir_all(local_tx)?;
    }
    // Server-side data, plus any leftover folders after a restore, needs the
    // server; without a session only purely local copies can be discarded.
    match (tx.side, session) {
        (Side::Remote, Some(s)) if s.fs()?.keeps_backups_on_server() => {
            let root = remote_tx_root(s, &tx.id)?;
            if s.fs()?.exists(&root).await? {
                s.fs()?.delete_tree(&root).await?;
            }
        }
        (Side::Remote, None) if tx.entries.iter().any(|e| e.stored_on == Side::Remote) => {
            return Err(AppError::SessionNotFound);
        }
        _ => {}
    }
    let _guard = INDEX.lock().unwrap();
    let mut txs = load_index()?;
    txs.retain(|t| t.id != tx.id);
    save_index(&txs)
}

pub async fn delete(id: &str, session: Option<&Session>) -> AppResult<()> {
    let tx = list()?.into_iter().find(|t| t.id == id).ok_or_else(|| AppError::other(tr!("Backup not found", "Backup niet gevonden")))?;
    discard(&tx, session).await
}

/// Drop transactions older than `days`. Server-side data can only be removed
/// while connected to that server; those wait until the next connection.
pub async fn purge(days: u32, session: Option<&Session>) -> AppResult<usize> {
    let cutoff = now_ms() - i64::from(days) * 24 * 3600 * 1000;
    let mut removed = 0;
    for tx in list()?.into_iter().filter(|t| t.created < cutoff) {
        let needs_server = tx.side == Side::Remote && tx.entries.iter().any(|e| e.stored_on == Side::Remote);
        let reachable = session.is_some_and(|s| tx.server_id.as_deref() == Some(s.server_id.as_str()));
        if needs_server && !reachable {
            continue;
        }
        discard(&tx, if needs_server { session } else { None }).await?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Delete a local folder into a backup, then restore it. Runs against a
    /// temporary XDG data dir so the real backup index is untouched.
    #[tokio::test]
    async fn local_delete_and_restore_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("kade-test-{}", uuid::Uuid::new_v4()));
        std::env::set_var("XDG_DATA_HOME", tmp.join("data"));
        let work = tmp.join("work/site");
        std::fs::create_dir_all(work.join("sub")).unwrap();
        std::fs::write(work.join("sub/a.txt"), "hello").unwrap();
        let path = work.to_string_lossy().into_owned();

        let mut rec = Recorder::new(Side::Local, Op::Delete, None, "test");
        rec.stash(None, &path).await.unwrap();
        let tx = rec.commit().unwrap().unwrap();
        assert!(!work.exists(), "original should be moved away");
        assert_eq!(list().unwrap().len(), 1);

        // Something new took its place; restoring must back that up too.
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("new.txt"), "x").unwrap();
        let undo = restore(&tx.id, None).await.unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(work.join("sub/a.txt")).unwrap(), "hello");
        assert!(!work.join("new.txt").exists());
        assert_eq!(undo.op, Op::Restore);
        assert!(list().unwrap().iter().any(|t| t.id == tx.id && t.restored));

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn mirrors_absolute_paths() {
        assert_eq!(mirrored("/home/j/.cache/kade/backups/t1", "/var/www/.env"), "/home/j/.cache/kade/backups/t1/var/www/.env");
    }

    #[test]
    fn parent_of_paths() {
        assert_eq!(parent("/var/www/x"), "/var/www");
        assert_eq!(parent("/x"), "/");
    }
}
