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
//!
//! While a transaction is being built, every item is also appended to a small
//! journal next to the index, so a crash mid-way leaves backups that can still
//! be listed, restored and purged. `commit` folds the journal into the index.

use std::collections::HashSet;
use std::io::Write as _;
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
    /// Something the action *added* at `original` (a sync's new file or
    /// folder); nothing is stored. Restoring moves it aside into the
    /// restore's own transaction instead of putting anything back.
    #[serde(default, skip_serializing_if = "is_false")]
    pub created: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl BackupEntry {
    /// An item the action created at `original`.
    pub fn created(original: String, is_dir: bool, side: Side) -> Self {
        BackupEntry { original, stored: String::new(), stored_on: side, is_dir, created: true }
    }
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

/// Tests change `KADE_DATA_HOME` / `KADE_CONFIG_HOME` for the whole process;
/// they hold this lock so parallel tests don't see each other's folders.
#[cfg(test)]
pub(crate) static TEST_ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One line of a transaction's journal.
#[derive(Debug, Serialize, Deserialize)]
enum Line {
    Begin(Transaction),
    Add(BackupEntry),
    /// The item stored at this path was put back.
    Drop(String),
}

/// `KADE_DATA_HOME` overrides the data folder. Tests rely on it: `dirs`
/// ignores `XDG_DATA_HOME` on macOS.
pub(crate) fn local_root() -> AppResult<PathBuf> {
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

fn journal_dir() -> AppResult<PathBuf> {
    Ok(local_root()?.join("journal"))
}

fn journal_path(id: &str) -> AppResult<PathBuf> {
    Ok(journal_dir()?.join(format!("{id}.jsonl")))
}

fn remove_journal(id: &str) {
    if let Ok(path) = journal_path(id) {
        let _ = std::fs::remove_file(path);
    }
}

/// Transactions that were never committed (the app quit mid-way), rebuilt from
/// their journals. A torn last line is ignored.
fn load_journals() -> Vec<Transaction> {
    let Ok(dir) = journal_dir() else { return Vec::new() };
    let Ok(files) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut txs = Vec::new();
    for file in files.flatten() {
        let Ok(text) = std::fs::read_to_string(file.path()) else { continue };
        let mut tx: Option<Transaction> = None;
        for line in text.lines().filter_map(|l| serde_json::from_str::<Line>(l).ok()) {
            match (line, tx.as_mut()) {
                (Line::Begin(t), _) => tx = Some(t),
                (Line::Add(e), Some(t)) => t.entries.push(e),
                (Line::Drop(stored), Some(t)) => t.entries.retain(|e| e.stored != stored),
                _ => {}
            }
        }
        txs.extend(tx.filter(|t| !t.entries.is_empty()));
    }
    txs
}

pub fn list() -> AppResult<Vec<Transaction>> {
    let _guard = INDEX.lock().unwrap();
    let mut txs = load_index()?;
    for tx in load_journals() {
        if !txs.iter().any(|t| t.id == tx.id) {
            txs.push(tx);
        }
    }
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

/// The folder on the server that holds Kade's own files (backups among them).
pub fn remote_kade_dir(session: &Session) -> Option<String> {
    session.remote_home.as_deref().map(|home| join_remote(home, parent(REMOTE_ROOT)))
}

/// `path` is `dir` or lies inside it (`/a/bc` is not inside `/a/b`).
pub(crate) fn is_within(path: &str, dir: &str) -> bool {
    let (path, dir) = (path.trim_end_matches('/'), dir.trim_end_matches('/'));
    path.strip_prefix(dir).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Refuse server paths whose backup would be catastrophic: the home folder,
/// and anything containing Kade's backup folder (it can't move into itself,
/// and the fallback would delete every backup on the server).
pub(crate) fn guard_remote(session: &Session, path: &str) -> AppResult<()> {
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
    /// Whether the journal has its header yet; also serialises journal writes.
    journal: Mutex<bool>,
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
            journal: Mutex::new(false),
        }
    }

    /// Append to the journal, writing the header first. A failure only costs
    /// crash safety, so it is logged rather than failing a stash that already happened.
    fn journal_line(&self, begun: &mut bool, line: Line) {
        let mut write = || -> AppResult<()> {
            std::fs::create_dir_all(journal_dir()?)?;
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(journal_path(&self.id)?)?;
            if !*begun {
                let mut head = self.tx.lock().unwrap().clone();
                head.entries.clear();
                writeln!(file, "{}", serde_json::to_string(&Line::Begin(head))?)?;
                *begun = true;
            }
            writeln!(file, "{}", serde_json::to_string(&line)?)?;
            Ok(())
        };
        if let Err(e) = write() {
            eprintln!("kade: backup-journal bijwerken mislukt: {e}");
        }
    }

    /// Add an item that was already moved into this transaction's folder (by
    /// rsync's `--backup-dir`, for instance).
    pub fn record(&self, entry: BackupEntry) {
        let mut begun = self.journal.lock().unwrap();
        self.journal_line(&mut begun, Line::Add(entry.clone()));
        self.tx.lock().unwrap().entries.push(entry);
    }

    /// Drop an entry whose stored copy turned out not to exist.
    fn forget(&self, stored: &str) {
        let mut begun = self.journal.lock().unwrap();
        self.tx.lock().unwrap().entries.retain(|e| e.stored != stored);
        if *begun {
            self.journal_line(&mut begun, Line::Drop(stored.to_string()));
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn side(&self) -> Side {
        self.side
    }

    /// Where this transaction keeps what is replaced under `dest_root`, laid out
    /// like [`stash`](Self::stash) does: `<dir>/rel` holds `dest_root/rel`.
    pub fn backup_dir(&self, session: Option<&Session>, dest_root: &str) -> AppResult<String> {
        match (self.side, session) {
            (Side::Remote, Some(s)) => Ok(mirrored(&remote_tx_root(s, &self.id)?, dest_root)),
            (Side::Remote, None) => Err(AppError::SessionNotFound),
            (Side::Local, _) => Ok(self.local_copy_path(dest_root)?.to_string_lossy().into_owned()),
        }
    }

    /// Make the transaction match what `backup_dir` really holds after another
    /// program filled it: files nobody recorded are added, recorded ones that
    /// aren't there are dropped. When it holds nothing, its folders are removed.
    pub async fn reconcile(&self, session: Option<&Session>, backup_dir: &str, dest_root: &str) -> AppResult<()> {
        let (files, dirs) = match (self.side, session) {
            (Side::Local, _) => walk_local(Path::new(backup_dir))?,
            (Side::Remote, Some(s)) => walk_remote(s, backup_dir).await?,
            (Side::Remote, None) => return Err(AppError::SessionNotFound),
        };
        let found: HashSet<&str> = files.iter().map(String::as_str).collect();
        let gone: Vec<String> = {
            let tx = self.tx.lock().unwrap();
            tx.entries
                .iter()
                .filter(|e| {
                    !e.created
                        && e.stored_on == self.side
                        && !e.is_dir
                        && is_within(&e.stored, backup_dir)
                        && !found.contains(e.stored.as_str())
                })
                .map(|e| e.stored.clone())
                .collect()
        };
        for stored in gone {
            self.forget(&stored);
        }
        for file in &files {
            let covered = self
                .tx
                .lock()
                .unwrap()
                .entries
                .iter()
                .any(|e| !e.created && (e.stored == *file || (e.is_dir && is_within(file, &e.stored))));
            let Some(rel) = file.strip_prefix(backup_dir).and_then(|r| r.strip_prefix('/')) else { continue };
            if !covered {
                let original = join_remote(dest_root, rel);
                self.record(BackupEntry { original, stored: file.clone(), stored_on: self.side, is_dir: false, created: false });
            }
        }
        if files.is_empty() && !dirs.is_empty() {
            self.remove_empty(session, dirs).await?;
        }
        Ok(())
    }

    /// Remove empty backup folders, deepest first, then their parents up to the transaction's root.
    async fn remove_empty(&self, session: Option<&Session>, mut dirs: Vec<String>) -> AppResult<()> {
        dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));
        match (self.side, session) {
            (Side::Remote, Some(s)) => {
                let root = remote_tx_root(s, &self.id)?;
                for dir in &dirs {
                    let _ = s.fs()?.remove_dir(dir).await;
                }
                if let Some(top) = dirs.last() {
                    s.fs()?.rmdir_up(parent(top), &root).await;
                }
            }
            _ => {
                let root = local_root()?.join(&self.id);
                for dir in &dirs {
                    let _ = std::fs::remove_dir(dir);
                }
                let mut up = dirs.last().map(|d| PathBuf::from(parent(d)));
                while let Some(dir) = up.filter(|d| d.starts_with(&root)) {
                    if std::fs::remove_dir(&dir).is_err() || dir == root {
                        break;
                    }
                    up = dir.parent().map(Path::to_path_buf);
                }
            }
        }
        Ok(())
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
                BackupEntry { original: path.into(), stored: dst.to_string_lossy().into(), stored_on: Side::Local, is_dir, created: false }
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
                    created: false,
                };
                if fs.keeps_backups_on_server() {
                    let root = remote_tx_root(session, &self.id)?;
                    let stored = mirrored(&root, path);
                    fs.mkdir_p_cached(parent(&stored), &mut *self.remote_dirs.lock().await).await?;
                    if fs.rename(path, &stored).await.is_ok() {
                        BackupEntry { original: path.into(), stored, stored_on: Side::Remote, is_dir, created: false }
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
        self.record(entry);
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
            let Some(entry) = tx.entries.iter().rfind(|e| !e.created && e.original == path) else { return Ok(()) };
            entry.clone()
        };
        move_back(self.side, session, &entry).await?;
        let now_empty = {
            let mut begun = self.journal.lock().unwrap();
            let now_empty = {
                let mut tx = self.tx.lock().unwrap();
                tx.entries.retain(|e| e.stored != entry.stored);
                tx.entries.is_empty()
            };
            if now_empty {
                remove_journal(&self.id);
                *begun = false;
            } else if *begun {
                self.journal_line(&mut begun, Line::Drop(entry.stored.clone()));
            }
            now_empty
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
        let mut tx = self.tx.into_inner().unwrap();
        prune_created(&mut tx.entries);
        if tx.entries.is_empty() {
            remove_journal(&tx.id);
            return Ok(None);
        }
        let _guard = INDEX.lock().unwrap();
        let mut txs = load_index()?;
        txs.retain(|t| t.id != tx.id);
        txs.push(tx.clone());
        save_index(&txs)?;
        remove_journal(&tx.id);
        Ok(Some(tx))
    }
}

/// A created folder already covers what was created inside it.
fn prune_created(entries: &mut Vec<BackupEntry>) {
    let dirs: Vec<String> = entries.iter().filter(|e| e.created && e.is_dir).map(|e| e.original.clone()).collect();
    entries.retain(|e| !e.created || !dirs.iter().any(|d| *d != e.original && is_within(&e.original, d)));
}

/// Files (and symlinks) and folders below `dir`, the folder itself included; empty when it doesn't exist.
fn walk_local(dir: &Path) -> AppResult<(Vec<String>, Vec<String>)> {
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    if std::fs::symlink_metadata(dir).is_err() {
        return Ok((files, dirs));
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                stack.push(entry.path());
            } else {
                files.push(entry.path().to_string_lossy().into_owned());
            }
        }
        dirs.push(d.to_string_lossy().into_owned());
    }
    Ok((files, dirs))
}

async fn walk_remote(session: &Session, dir: &str) -> AppResult<(Vec<String>, Vec<String>)> {
    let fs = session.fs()?;
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    if fs.stat(dir).await?.is_none() {
        return Ok((files, dirs));
    }
    let mut stack = vec![dir.to_string()];
    while let Some(d) = stack.pop() {
        for entry in fs.list(&d).await? {
            if entry.is_dir && !entry.is_symlink {
                stack.push(entry.path);
            } else {
                files.push(entry.path);
            }
        }
        dirs.push(d);
    }
    Ok((files, dirs))
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

    if tx.restored {
        return Err(AppError::other(tr!("This backup was already restored", "Deze backup is al teruggezet")));
    }

    let undoable_adds = tx.entries.iter().any(|e| e.created);
    let undo = Recorder::new(tx.side, Op::Restore, session, tr!("Restoring: {}", "Terugzetten van: {}", tx.summary));
    let moved = async {
        for entry in &tx.entries {
            let occupied = exists(tx.side, session, &entry.original).await?;
            if occupied {
                undo.stash(session, &entry.original).await?;
            }
            if !entry.created {
                move_back(tx.side, session, entry).await?;
                if !occupied && undoable_adds {
                    // Only for a transaction that added things (a sync): undoing its restore
                    // must take this back out again. A plain delete's restore stays as it was.
                    undo.record(BackupEntry::created(entry.original.clone(), entry.is_dir, tx.side));
                }
            }
        }
        AppResult::Ok(())
    }
    .await;
    if let Err(e) = moved {
        // What was replaced so far stays restorable.
        if let Err(err) = undo.commit() {
            eprintln!("kade: backup-index bijwerken mislukt: {err}");
        }
        return Err(e);
    }

    {
        let _guard = INDEX.lock().unwrap();
        let mut txs = load_index()?;
        match txs.iter_mut().find(|t| t.id == id) {
            Some(t) => t.restored = true,
            // Only in a journal (the app quit before it was committed): adopt it.
            None => txs.push(Transaction { restored: true, ..tx.clone() }),
        }
        save_index(&txs)?;
        remove_journal(id);
    }
    undo.commit()
}

/// Commit a recorder after a failure, keeping the original error. For
/// multi-item actions: what was stashed before the failure stays restorable.
pub fn commit_after<T>(undo: Recorder, result: AppResult<T>) -> AppResult<Option<Transaction>> {
    match result {
        Ok(_) => undo.commit(),
        Err(e) => {
            if let Err(err) = undo.commit() {
                eprintln!("kade: backup-index bijwerken mislukt: {err}");
            }
            Err(e)
        }
    }
}

/// Remove a transaction's stored data and its index entry.
async fn discard(tx: &Transaction, session: Option<&Session>) -> AppResult<()> {
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
    let local_tx = local_root()?.join(&tx.id);
    if local_tx.exists() {
        std::fs::remove_dir_all(local_tx)?;
    }
    // Last, so a failure above leaves the transaction listed and retryable.
    let _guard = INDEX.lock().unwrap();
    let mut txs = load_index()?;
    txs.retain(|t| t.id != tx.id);
    save_index(&txs)?;
    remove_journal(&tx.id);
    Ok(())
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
        match discard(&tx, if needs_server { session } else { None }).await {
            Ok(()) => removed += 1,
            Err(e) => eprintln!("kade: backup {} opruimen mislukt: {e}", tx.id),
        }
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
        let _env = TEST_ENV.lock().await;
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

    fn temp_data() -> PathBuf {
        let tmp = std::env::temp_dir().join(format!("kade-test-{}", uuid::Uuid::new_v4()));
        std::env::set_var("KADE_DATA_HOME", tmp.join("data"));
        tmp
    }

    #[tokio::test]
    async fn restoring_twice_is_refused() {
        let _env = TEST_ENV.lock().await;
        let tmp = temp_data();
        let file = tmp.join("work/a.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "one").unwrap();

        let rec = Recorder::new(Side::Local, Op::Delete, None, "test");
        rec.stash(None, &file.to_string_lossy()).await.unwrap();
        let tx = rec.commit().unwrap().unwrap();
        restore(&tx.id, None).await.unwrap();

        // The file was put back; a second restore must not touch it.
        assert!(restore(&tx.id, None).await.is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one");
        assert_eq!(list().unwrap().len(), 1, "no extra transaction from the refused restore");

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn failed_multi_delete_stays_restorable() {
        let _env = TEST_ENV.lock().await;
        let tmp = temp_data();
        let a = tmp.join("work/a.txt");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::write(&a, "a").unwrap();
        let missing = tmp.join("work/missing.txt");

        let rec = Recorder::new(Side::Local, Op::Delete, None, "test");
        let result = async {
            rec.stash(None, &a.to_string_lossy()).await?;
            rec.stash(None, &missing.to_string_lossy()).await
        }
        .await;
        assert!(result.is_err());
        assert!(commit_after(rec, result).is_err());

        let txs = list().unwrap();
        assert_eq!(txs.len(), 1);
        assert!(!a.exists());
        restore(&txs[0].id, None).await.unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn uncommitted_transaction_is_listed_from_its_journal() {
        let _env = TEST_ENV.lock().await;
        let tmp = temp_data();
        let a = tmp.join("work/a.txt");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::write(&a, "a").unwrap();

        // Never committed, as after a crash.
        let rec = Recorder::new(Side::Local, Op::Overwrite, None, "test");
        rec.stash(None, &a.to_string_lossy()).await.unwrap();
        let id = rec.id().to_string();
        drop(rec);

        let txs = list().unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].id, id);
        restore(&id, None).await.unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
        assert!(list().unwrap().iter().any(|t| t.id == id && t.restored));

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn purge_continues_past_a_failure() {
        let _env = TEST_ENV.lock().await;
        let tmp = temp_data();
        let mut ids = Vec::new();
        for name in ["a", "b"] {
            let f = tmp.join(format!("work/{name}.txt"));
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, name).unwrap();
            let rec = Recorder::new(Side::Local, Op::Delete, None, name);
            rec.stash(None, &f.to_string_lossy()).await.unwrap();
            ids.push(rec.commit().unwrap().unwrap().id);
        }
        // Make the first one undiscardable: its data folder is a plain file, which remove_dir_all rejects.
        let blocked = local_root().unwrap().join(&ids[0]);
        std::fs::remove_dir_all(&blocked).unwrap();
        std::fs::write(&blocked, "").unwrap();
        let removed = purge(0, None).await.unwrap();
        assert_eq!(removed, 1, "the failing one is skipped, the other still purged");
        assert_eq!(list().unwrap().iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), [ids[0].as_str()]);

        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// A sync's backup folder: rsync fills it, `reconcile` makes the transaction match.
    #[tokio::test]
    async fn reconcile_matches_the_backup_folder() {
        let _env = TEST_ENV.lock().await;
        let tmp = temp_data();
        let dest = tmp.join("site");
        std::fs::create_dir_all(dest.join("a")).unwrap();
        let dest = dest.to_string_lossy().into_owned();
        let rec = Recorder::new(Side::Local, Op::Overwrite, None, "sync");
        let dir = rec.backup_dir(None, &dest).unwrap();
        // Same layout as a stash of the same file.
        assert_eq!(Path::new(&dir).join("a/b.txt"), rec.local_copy_path(&format!("{dest}/a/b.txt")).unwrap());

        // What rsync left: two replaced files. One was streamed, one line was
        // lost; one streamed line never got its file.
        std::fs::create_dir_all(Path::new(&dir).join("a")).unwrap();
        std::fs::write(Path::new(&dir).join("a/b.txt"), "old b").unwrap();
        std::fs::write(Path::new(&dir).join("c.txt"), "old c").unwrap();
        let entry = |rel: &str| BackupEntry {
            original: format!("{dest}/{rel}"),
            stored: format!("{dir}/{rel}"),
            stored_on: Side::Local,
            is_dir: false,
            created: false,
        };
        rec.record(entry("c.txt"));
        rec.record(entry("never.txt"));
        rec.reconcile(None, &dir, &dest).await.unwrap();
        rec.reconcile(None, &dir, &dest).await.unwrap();
        let mut originals: Vec<String> = rec.tx.lock().unwrap().entries.iter().map(|e| e.original.clone()).collect();
        originals.sort();
        assert_eq!(originals, [format!("{dest}/a/b.txt"), format!("{dest}/c.txt")]);

        // The new versions are in place; restoring brings the old ones back.
        std::fs::write(format!("{dest}/a/b.txt"), "new b").unwrap();
        std::fs::write(format!("{dest}/c.txt"), "new c").unwrap();
        let tx = rec.commit().unwrap().unwrap();
        restore(&tx.id, None).await.unwrap();
        assert_eq!(std::fs::read_to_string(format!("{dest}/a/b.txt")).unwrap(), "old b");
        assert_eq!(std::fs::read_to_string(format!("{dest}/c.txt")).unwrap(), "old c");
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn restore_moves_added_items_aside_and_can_be_undone() {
        let _env = TEST_ENV.lock().await;
        let tmp = temp_data();
        let dest = tmp.join("site");
        std::fs::create_dir_all(&dest).unwrap();
        let dest = dest.to_string_lossy().into_owned();
        let rec = Recorder::new(Side::Local, Op::Overwrite, None, "sync");
        let dir = rec.backup_dir(None, &dest).unwrap();
        // The sync replaced old.txt (backed up), added new.txt and a folder with a file.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Path::new(&dir).join("old.txt"), "before").unwrap();
        std::fs::write(format!("{dest}/old.txt"), "after").unwrap();
        std::fs::write(format!("{dest}/new.txt"), "added").unwrap();
        std::fs::create_dir_all(format!("{dest}/fresh")).unwrap();
        std::fs::write(format!("{dest}/fresh/x.txt"), "added").unwrap();
        rec.record(BackupEntry::created(format!("{dest}/fresh/x.txt"), false, Side::Local));
        rec.record(BackupEntry::created(format!("{dest}/new.txt"), false, Side::Local));
        rec.record(BackupEntry::created(format!("{dest}/fresh"), true, Side::Local));
        rec.reconcile(None, &dir, &dest).await.unwrap();
        let tx = rec.commit().unwrap().unwrap();
        // The folder covers its file; reconcile didn't mistake a created folder for a backup.
        assert_eq!(tx.entries.iter().filter(|e| e.created).count(), 2);
        assert_eq!(tx.entries.len(), 3);

        let undo = restore(&tx.id, None).await.unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(format!("{dest}/old.txt")).unwrap(), "before");
        assert!(!Path::new(&format!("{dest}/new.txt")).exists());
        assert!(!Path::new(&format!("{dest}/fresh")).exists());

        // Restoring the restore brings the sync's result back, additions included.
        restore(&undo.id, None).await.unwrap();
        assert_eq!(std::fs::read_to_string(format!("{dest}/old.txt")).unwrap(), "after");
        assert_eq!(std::fs::read_to_string(format!("{dest}/new.txt")).unwrap(), "added");
        assert_eq!(std::fs::read_to_string(format!("{dest}/fresh/x.txt")).unwrap(), "added");
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn entries_without_the_created_flag_still_parse() {
        let old = r#"{"original":"/a","stored":"/b","stored_on":"local","is_dir":false}"#;
        assert!(!serde_json::from_str::<BackupEntry>(old).unwrap().created);
        let json = serde_json::to_string(&BackupEntry::created("/a".into(), false, Side::Remote)).unwrap();
        assert!(json.contains("\"created\":true"));
        assert!(!serde_json::to_string(&serde_json::from_str::<BackupEntry>(old).unwrap()).unwrap().contains("created"));
    }

    #[tokio::test]
    async fn reconcile_of_an_empty_backup_leaves_nothing() {
        let _env = TEST_ENV.lock().await;
        let tmp = temp_data();
        let dest = tmp.join("site").to_string_lossy().into_owned();
        let rec = Recorder::new(Side::Local, Op::Overwrite, None, "sync");
        let dir = rec.backup_dir(None, &dest).unwrap();
        std::fs::create_dir_all(Path::new(&dir).join("empty/sub")).unwrap();
        rec.reconcile(None, &dir, &dest).await.unwrap();
        assert!(!local_root().unwrap().join(rec.id()).exists(), "the transaction's folder is removed");
        assert!(rec.commit().unwrap().is_none());
        // A backup folder that was never made is fine too.
        let rec = Recorder::new(Side::Local, Op::Overwrite, None, "sync");
        rec.reconcile(None, &rec.backup_dir(None, &dest).unwrap(), &dest).await.unwrap();
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn remote_backup_dir_mirrors_the_destination() {
        let session = Session { ssh: None, fs: None, server_id: "s".into(), server_name: "s".into(), remote_home: Some("/home/j".into()) };
        let rec = Recorder::new(Side::Remote, Op::Overwrite, Some(&session), "sync");
        let dir = rec.backup_dir(Some(&session), "/var/www/site").unwrap();
        let root = remote_tx_root(&session, rec.id()).unwrap();
        assert_eq!(join_remote(&dir, "a/b.txt"), mirrored(&root, "/var/www/site/a/b.txt"));
        assert_eq!(remote_kade_dir(&session).as_deref(), Some("/home/j/.cache/kade"));
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
