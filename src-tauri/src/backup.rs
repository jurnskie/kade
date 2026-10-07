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
use tokio::io::AsyncWriteExt;

use crate::error::{AppError, AppResult};
use crate::remote::{copy_chunks, no_op};
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

/// `KADE_DATA_HOME` overrides the data folder. Tests rely on it: `dirs`
/// ignores `XDG_DATA_HOME` on macOS.
fn local_root() -> AppResult<PathBuf> {
    let dir = std::env::var_os("KADE_DATA_HOME")
        .map(PathBuf::from)
        .or_else(dirs::data_local_dir)
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
    session.remote_home.as_deref().is_some_and(|home| is_within(path, &join_remote(home, REMOTE_ROOT)))
}

/// `path` is `dir` or lies inside it (`/a/bc` is not inside `/a/b`).
fn is_within(path: &str, dir: &str) -> bool {
    let (path, dir) = (path.trim_end_matches('/'), dir.trim_end_matches('/'));
    path.strip_prefix(dir).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Refuse server paths whose backup would be catastrophic: the home folder,
/// and anything containing Kade's backup folder (it can't move into itself,
/// and the fallback would delete every backup on the server).
fn guard_remote(session: &Session, path: &str) -> AppResult<()> {
    crate::fs::guard(path)?;
    let Some(home) = session.remote_home.as_deref() else { return Ok(()) };
    if is_within(home, path) || is_within(&join_remote(home, REMOTE_ROOT), path) {
        return Err(AppError::other(tr!("Refusing to operate on {path}", "Weigert bewerking op {path}", path = path)));
    }
    Ok(())
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

async fn download_tree(session: &Session, src: &str, dst: &Path, is_dir: bool) -> AppResult<()> {
    let fs = session.fs()?;
    if !is_dir {
        return download_file(fs, src, dst).await;
    }
    // The listing says what each child is, so no extra request per entry.
    let mut stack = vec![(src.to_string(), dst.to_path_buf())];
    while let Some((dir, local)) = stack.pop() {
        std::fs::create_dir_all(&local)?;
        for entry in fs.list(&dir).await? {
            let to = local.join(&entry.name);
            match (entry.is_dir, entry.is_symlink) {
                (true, false) => stack.push((entry.path, to)),
                // Neither FTP nor this backup can recreate a link, and following it
                // could copy half the server; the link is left out of the backup.
                (true, true) => eprintln!("kade: not backing up symlinked folder {}", entry.path),
                (false, _) => download_file(fs, &entry.path, &to).await?,
            }
        }
    }
    Ok(())
}

async fn download_file(fs: &crate::remote::RemoteFs, src: &str, dst: &Path) -> AppResult<()> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut from = fs.reader(src).await?;
    let mut to = tokio::fs::File::create(dst).await?;
    copy_chunks(&mut from, &mut to, no_op).await?;
    from.finish().await?;
    to.flush().await?;
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
            copy_chunks(&mut from, &mut to, no_op).await?;
            to.finish().await?;
        }
    }
    Ok(())
}

/// Collects the items of one transaction as they are moved aside. Several
/// files may be stashed at once (a transfer copies files in parallel).
pub struct Recorder {
    id: String,
    side: Side,
    tx: Mutex<Transaction>,
    /// Remote backup folders known to exist, so each file's stash doesn't re-check the whole path.
    remote_dirs: tokio::sync::Mutex<HashSet<String>>,
}

impl Recorder {
    pub fn new(side: Side, op: Op, session: Option<&Session>, summary: impl Into<String>) -> Self {
        let id = format!("{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), &uuid::Uuid::new_v4().to_string()[..8]);
        Recorder {
            tx: Mutex::new(Transaction {
                id: id.clone(),
                created: now_ms(),
                side,
                op,
                summary: summary.into(),
                server_id: session.map(|s| s.server_id.clone()),
                server_name: session.map(|s| s.server_name.clone()),
                entries: Vec::new(),
                restored: false,
            }),
            id,
            side,
            remote_dirs: Default::default(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn local_copy_path(&self, path: &str) -> AppResult<PathBuf> {
        Ok(local_root()?.join(&self.id).join(path.trim_start_matches('/')))
    }

    /// Move `path` into the backup. Afterwards `path` no longer exists.
    pub async fn stash(&self, session: Option<&Session>, path: &str) -> AppResult<()> {
        self.stash_known(session, path, None).await
    }

    /// [`stash`](Self::stash), with `is_dir` passed in when the caller already
    /// knows it (saves a request per file).
    pub async fn stash_known(&self, session: Option<&Session>, path: &str, is_dir: Option<bool>) -> AppResult<()> {
        match (self.side, session) {
            (Side::Remote, Some(session)) => guard_remote(session, path)?,
            _ => crate::fs::guard(path)?,
        }
        let entry = match (self.side, session) {
            (Side::Local, _) => {
                let src = Path::new(path);
                let is_dir = std::fs::symlink_metadata(src)?.is_dir();
                let dst = self.local_copy_path(path)?;
                move_local(src, &dst)?;
                BackupEntry { original: path.into(), stored: dst.to_string_lossy().into(), stored_on: Side::Local, is_dir }
            }
            (Side::Remote, Some(session)) => {
                let fs = session.fs()?;
                let is_dir = match is_dir {
                    Some(d) => d,
                    None => fs.is_real_dir(path).await?,
                };
                let local_copy = |dst: &Path| BackupEntry {
                    original: path.into(),
                    stored: dst.to_string_lossy().into(),
                    stored_on: Side::Local,
                    is_dir,
                };
                if fs.keeps_backups_on_server() {
                    let root = remote_tx_root(session, &self.id)?;
                    let stored = mirrored(&root, path);
                    fs.mkdir_p_cached(parent(&stored), &mut *self.remote_dirs.lock().await).await?;
                    if fs.rename(path, &stored).await.is_ok() {
                        BackupEntry { original: path.into(), stored, stored_on: Side::Remote, is_dir }
                    } else {
                        // Different filesystem on the server: keep a local copy instead,
                        // and drop the now-unused folders we just created for it.
                        fs.rmdir_up(parent(&stored), &root).await;
                        self.remote_dirs.lock().await.clear();
                        local_copy(&self.download_and_delete(session, path, is_dir).await?)
                    }
                } else {
                    local_copy(&self.download_and_delete(session, path, is_dir).await?)
                }
            }
            (Side::Remote, None) => return Err(AppError::SessionNotFound),
        };
        self.tx.lock().unwrap().entries.push(entry);
        Ok(())
    }

    /// Keep a local copy of a server item, then remove it from the server.
    async fn download_and_delete(&self, session: &Session, path: &str, is_dir: bool) -> AppResult<PathBuf> {
        let dst = self.local_copy_path(path)?;
        download_tree(session, path, &dst, is_dir).await?;
        let fs = session.fs()?;
        if is_dir {
            fs.delete_tree(path).await?;
        } else {
            fs.remove_file(path).await?;
        }
        Ok(dst)
    }

    /// Undo the stash of `path`, for when what should have replaced it failed.
    /// On error the entry stays in the transaction, so it can still be restored.
    pub async fn put_back(&self, session: Option<&Session>, path: &str) -> AppResult<()> {
        let entry = {
            let tx = self.tx.lock().unwrap();
            let Some(entry) = tx.entries.iter().rfind(|e| e.original == path) else { return Ok(()) };
            entry.clone()
        };
        move_back(self.side, session, &entry).await?;
        let now_empty = {
            let mut tx = self.tx.lock().unwrap();
            tx.entries.retain(|e| e.stored != entry.stored);
            tx.entries.is_empty()
        };
        if let (Side::Remote, Some(s)) = (entry.stored_on, session) {
            // Drop the backup folders that are now empty.
            let root = remote_tx_root(s, &self.id)?;
            s.fs()?.rmdir_up(parent(&entry.stored), &root).await;
            self.remote_dirs.lock().await.clear();
        }
        let local_tx = local_root()?.join(&self.id);
        if now_empty && local_tx.exists() {
            std::fs::remove_dir_all(local_tx)?;
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.tx.lock().unwrap().entries.is_empty()
    }

    /// Record the transaction (no-op when nothing was backed up).
    pub fn commit(self) -> AppResult<Option<Transaction>> {
        let tx = self.tx.into_inner().unwrap();
        if tx.entries.is_empty() {
            return Ok(None);
        }
        let _guard = INDEX.lock().unwrap();
        let mut txs = load_index()?;
        txs.push(tx.clone());
        save_index(&txs)?;
        Ok(Some(tx))
    }
}

async fn exists(side: Side, session: Option<&Session>, path: &str) -> AppResult<bool> {
    Ok(match (side, session) {
        (Side::Local, _) => std::fs::symlink_metadata(path).is_ok(),
        (Side::Remote, Some(s)) => s.fs()?.exists(path).await?,
        (Side::Remote, None) => return Err(AppError::SessionNotFound),
    })
}

/// Move one stored item back to its original path.
async fn move_back(side: Side, session: Option<&Session>, entry: &BackupEntry) -> AppResult<()> {
    match (side, entry.stored_on) {
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
    Ok(())
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

    let undo = Recorder::new(tx.side, Op::Restore, session, tr!("Restoring: {}", "Terugzetten van: {}", tx.summary));
    for entry in &tx.entries {
        if exists(tx.side, session, &entry.original).await? {
            undo.stash(session, &entry.original).await?;
        }
        move_back(tx.side, session, entry).await?;
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
    /// temporary data dir so the real backup index is untouched.
    #[tokio::test]
    async fn local_delete_and_restore_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("kade-test-{}", uuid::Uuid::new_v4()));
        std::env::set_var("KADE_DATA_HOME", tmp.join("data"));
        let work = tmp.join("work/site");
        std::fs::create_dir_all(work.join("sub")).unwrap();
        std::fs::write(work.join("sub/a.txt"), "hello").unwrap();
        let path = work.to_string_lossy().into_owned();

        let rec = Recorder::new(Side::Local, Op::Delete, None, "test");
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
    fn within_respects_segments() {
        assert!(is_within("/home/j/.cache/kade/backups/t1/x", "/home/j/.cache/kade/backups"));
        assert!(is_within("/home/j/.cache/kade/backups/", "/home/j/.cache/kade/backups"));
        assert!(!is_within("/home/j/.cache/kade/backups-old/x", "/home/j/.cache/kade/backups"));
        assert!(!is_within("/home/j", "/home/j/.cache"));
    }

    #[test]
    fn parent_of_paths() {
        assert_eq!(parent("/var/www/x"), "/var/www");
        assert_eq!(parent("/x"), "/");
    }
}
