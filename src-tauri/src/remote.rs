//! One file-system interface over SFTP and FTP, so listing, transfers,
//! deleting and backups don't care which protocol a server speaks.

use std::collections::HashSet;
use std::future::{self, Future};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::fs::File as SftpFile;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::error::{AppError, AppResult};
use crate::fs::{format_mode, Entry};
use crate::ftp::{Ftp, FtpTransfer};
use crate::ssh::join_remote;

const CHUNK: usize = 256 * 1024;

/// What the transfer and backup code needs to know about a remote item.
#[derive(Debug, Clone)]
pub struct Meta {
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub mtime: Option<i64>,
}

/// Mode and owner of a file that is about to be overwritten. A new file would
/// otherwise come out with the server's defaults (e.g. a private 0600 file
/// turning world-readable, or an executable losing its bit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
}

impl Kept {
    fn from_sftp(m: &FileAttributes) -> Option<Kept> {
        let kept = Kept { mode: m.permissions.map(|p| p & 0o7777), uid: m.uid, gid: m.gid };
        (kept.mode.is_some() || kept.uid.is_some() || kept.gid.is_some()).then_some(kept)
    }

    fn attrs(&self, with_owner: bool) -> FileAttributes {
        let mut attrs = FileAttributes::empty();
        attrs.permissions = self.mode;
        if with_owner {
            attrs.uid = self.uid;
            attrs.gid = self.gid;
        }
        attrs
    }
}

pub enum RemoteFs {
    Sftp(SftpSession),
    Ftp(Ftp),
}

pub enum RemoteReader {
    Sftp(SftpFile),
    Ftp(FtpTransfer),
}

pub enum RemoteWriter {
    Sftp(SftpFile),
    Ftp(FtpTransfer),
}

impl AsyncRead for RemoteReader {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            RemoteReader::Sftp(f) => Pin::new(f).poll_read(cx, buf),
            RemoteReader::Ftp(t) => Pin::new(t).poll_read(cx, buf),
        }
    }
}

impl RemoteReader {
    pub async fn finish(self) -> AppResult<()> {
        match self {
            RemoteReader::Sftp(_) => Ok(()),
            RemoteReader::Ftp(t) => t.finish().await,
        }
    }
}

impl AsyncWrite for RemoteWriter {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            RemoteWriter::Sftp(f) => Pin::new(f).poll_write(cx, buf),
            RemoteWriter::Ftp(t) => Pin::new(t).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            RemoteWriter::Sftp(f) => Pin::new(f).poll_flush(cx),
            RemoteWriter::Ftp(t) => Pin::new(t).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            RemoteWriter::Sftp(f) => Pin::new(f).poll_shutdown(cx),
            RemoteWriter::Ftp(t) => Pin::new(t).poll_shutdown(cx),
        }
    }
}

impl RemoteWriter {
    pub async fn finish(self) -> AppResult<()> {
        match self {
            RemoteWriter::Sftp(mut f) => Ok(f.shutdown().await?),
            RemoteWriter::Ftp(t) => t.finish().await,
        }
    }
}

/// Copy `src` into `dst` in large chunks; `after` runs once per chunk with its
/// size (progress), and can wait (pause) or stop the copy with an error (cancel).
// A plain closure returning a future, not an async closure: those break the
// `Send` check of the spawned transfer job.
pub async fn copy_chunks<R, W, F>(src: &mut R, dst: &mut W, mut after: impl FnMut(usize) -> F) -> AppResult<()>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
    F: Future<Output = AppResult<()>>,
{
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = src.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        dst.write_all(&buf[..n]).await?;
        after(n).await?;
    }
}

/// For `copy_chunks` without anything to do between chunks.
pub fn no_op(_: usize) -> future::Ready<AppResult<()>> {
    future::ready(Ok(()))
}

fn meta_from_sftp(m: &FileAttributes) -> Meta {
    Meta { is_dir: m.is_dir(), is_symlink: m.file_type().is_symlink(), size: m.size.unwrap_or(0), mtime: m.mtime.map(i64::from) }
}

/// A single path segment: no separators, not `.` or `..`.
fn is_plain_name(name: &str) -> bool {
    !matches!(name, "" | "." | "..") && !name.contains(['/', '\0'])
}

async fn sftp_list(sftp: &SftpSession, dir: &str) -> AppResult<Vec<Entry>> {
    let mut out = Vec::new();
    let mut links = Vec::new();
    for item in sftp.read_dir(dir).await? {
        let name = item.file_name();
        let meta = item.metadata();
        if meta.file_type().is_symlink() {
            links.push(out.len());
        }
        out.push(Entry {
            path: join_remote(dir, &name),
            name,
            is_dir: meta.is_dir(),
            is_symlink: meta.file_type().is_symlink(),
            size: meta.size.unwrap_or(0),
            modified: meta.mtime.map(i64::from),
            permissions: meta.permissions.map(format_mode),
        });
    }
    // read_dir reports the links themselves; follow them to know which lead to a
    // directory. All at once: SFTP pipelines the requests.
    let targets = futures_util::future::join_all(links.iter().map(|&i| sftp.metadata(out[i].path.clone()))).await;
    for (i, target) in links.into_iter().zip(targets) {
        out[i].is_dir = target.is_ok_and(|m| m.is_dir());
    }
    Ok(out)
}

impl RemoteFs {
    /// Backups may be kept on the server only where moving files there is
    /// cheap and private: SFTP into the user's home. FTP roots are often the
    /// web root itself, so FTP backups always go to this computer instead.
    pub fn keeps_backups_on_server(&self) -> bool {
        matches!(self, RemoteFs::Sftp(_))
    }

    /// How many files one transfer job may copy at once. SFTP pipelines
    /// requests over one channel; FTP has a single control connection.
    pub fn parallel_files(&self) -> usize {
        match self {
            RemoteFs::Sftp(_) => 8,
            RemoteFs::Ftp(_) => 1,
        }
    }

    /// How far apart modification times may be and still count as equal.
    pub fn mtime_tolerance(&self) -> i64 {
        match self {
            RemoteFs::Ftp(ftp) => ftp.mtime_tolerance(),
            RemoteFs::Sftp(_) => 0,
        }
    }

    pub async fn list(&self, dir: &str) -> AppResult<Vec<Entry>> {
        let mut entries = match self {
            RemoteFs::Ftp(ftp) => ftp.list(dir).await?,
            RemoteFs::Sftp(sftp) => sftp_list(sftp, dir).await?,
        };
        // A broken or hostile server could send `../x` or `/etc/x`, which a
        // download would then write outside its destination.
        entries.retain(|e| is_plain_name(&e.name));
        Ok(entries)
    }

    /// Metadata following symlinks; `None` when the path does not exist.
    pub async fn stat(&self, path: &str) -> AppResult<Option<Meta>> {
        match self {
            RemoteFs::Ftp(ftp) => ftp.stat(path).await,
            // One STAT; "no such file" means absent (try_exists would be a second STAT).
            RemoteFs::Sftp(sftp) => match sftp.metadata(path).await {
                Ok(m) => Ok(Some(meta_from_sftp(&m))),
                Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(None),
                Err(e) => Err(e.into()),
            },
        }
    }

    /// Is this a real directory (not a symlink to one)? Used before deleting.
    pub async fn is_real_dir(&self, path: &str) -> AppResult<bool> {
        match self {
            RemoteFs::Ftp(ftp) => Ok(ftp.stat(path).await?.is_some_and(|m| m.is_dir && !m.is_symlink)),
            RemoteFs::Sftp(sftp) => Ok(sftp.symlink_metadata(path).await?.is_dir()),
        }
    }

    pub async fn exists(&self, path: &str) -> AppResult<bool> {
        Ok(self.stat(path).await?.is_some())
    }

    pub async fn mkdir(&self, path: &str) -> AppResult<()> {
        match self {
            RemoteFs::Ftp(ftp) => ftp.mkdir(path).await,
            RemoteFs::Sftp(sftp) => Ok(sftp.create_dir(path).await?),
        }
    }

    pub async fn mkdir_p(&self, dir: &str) -> AppResult<()> {
        self.mkdir_p_cached(dir, &mut HashSet::new()).await
    }

    /// `mkdir_p` that skips the directories in `known` (and adds the ones it
    /// checks), so a run of files under the same tree doesn't re-check every level.
    pub async fn mkdir_p_cached(&self, dir: &str, known: &mut HashSet<String>) -> AppResult<()> {
        let mut acc = String::new();
        for part in dir.split('/').filter(|p| !p.is_empty()) {
            acc = format!("{acc}/{part}");
            if known.contains(&acc) {
                continue;
            }
            if !self.exists(&acc).await? {
                self.mkdir(&acc).await?;
            }
            known.insert(acc.clone());
        }
        Ok(())
    }

    pub async fn remove_file(&self, path: &str) -> AppResult<()> {
        match self {
            RemoteFs::Ftp(ftp) => ftp.remove_file(path).await,
            RemoteFs::Sftp(sftp) => Ok(sftp.remove_file(path).await?),
        }
    }

    pub async fn remove_dir(&self, path: &str) -> AppResult<()> {
        match self {
            RemoteFs::Ftp(ftp) => ftp.remove_dir(path).await,
            RemoteFs::Sftp(sftp) => Ok(sftp.remove_dir(path).await?),
        }
    }

    pub async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        match self {
            RemoteFs::Ftp(ftp) => ftp.rename(from, to).await,
            RemoteFs::Sftp(sftp) => Ok(sftp.rename(from, to).await?),
        }
    }

    /// Create an empty file, failing if it already exists.
    pub async fn create_new(&self, path: &str) -> AppResult<()> {
        match self {
            RemoteFs::Sftp(sftp) => {
                sftp.open_with_flags(path, OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE).await?;
                Ok(())
            }
            RemoteFs::Ftp(_) => {
                // FTP has no exclusive create; check first.
                if self.exists(path).await? {
                    return Err(AppError::other(format!("{path} bestaat al")));
                }
                self.writer(path).await?.finish().await
            }
        }
    }

    /// Best effort: keep the source's modification time on the copy.
    pub async fn set_mtime(&self, path: &str, unix_secs: i64) {
        match self {
            RemoteFs::Ftp(ftp) => ftp.set_mtime(path, unix_secs).await,
            RemoteFs::Sftp(sftp) => {
                let mut attrs = FileAttributes::empty();
                attrs.mtime = Some(unix_secs as u32);
                attrs.atime = Some(unix_secs as u32);
                let _ = sftp.set_metadata(path, attrs).await;
            }
        }
    }

    pub async fn reader(&self, path: &str) -> AppResult<RemoteReader> {
        match self {
            RemoteFs::Ftp(ftp) => Ok(RemoteReader::Ftp(ftp.reader(path).await?)),
            RemoteFs::Sftp(sftp) => Ok(RemoteReader::Sftp(sftp.open(path).await?)),
        }
    }

    /// Open for writing, creating or truncating.
    pub async fn writer(&self, path: &str) -> AppResult<RemoteWriter> {
        match self {
            RemoteFs::Ftp(ftp) => Ok(RemoteWriter::Ftp(ftp.writer(path).await?)),
            RemoteFs::Sftp(sftp) => {
                Ok(RemoteWriter::Sftp(sftp.open_with_flags(path, OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE).await?))
            }
        }
    }

    /// Where an overwrite of `path` has to land, and what to restore on the new
    /// file. A symlink is followed, so the target is replaced (and backed up)
    /// instead of the link. FTP can't read modes reliably, so it keeps `path`.
    pub async fn overwrite_target(&self, path: &str) -> AppResult<(String, Option<Kept>)> {
        let RemoteFs::Sftp(sftp) = self else { return Ok((path.to_string(), None)) };
        let attrs = match sftp.symlink_metadata(path).await {
            Ok(m) => m,
            Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => return Ok((path.to_string(), None)),
            Err(e) => return Err(e.into()),
        };
        if !attrs.file_type().is_symlink() {
            return Ok((path.to_string(), Kept::from_sftp(&attrs)));
        }
        // A dangling link has no target to resolve; writing creates it as before.
        let Ok(target) = sftp.canonicalize(path).await else { return Ok((path.to_string(), None)) };
        let kept = sftp.metadata(target.clone()).await.ok().and_then(|m| Kept::from_sftp(&m));
        Ok((target, kept))
    }

    /// Best effort: give a freshly written file its old mode and owner back.
    /// Changing the owner usually needs root, so a refusal there still keeps the mode.
    pub async fn restore_kept(&self, path: &str, kept: &Kept) {
        if let RemoteFs::Sftp(sftp) = self {
            if sftp.set_metadata(path, kept.attrs(true)).await.is_err() && kept.mode.is_some() {
                let _ = sftp.set_metadata(path, kept.attrs(false)).await;
            }
        }
    }

    /// Delete a file, symlink or directory tree. Symlinks are removed, never followed.
    pub async fn delete_tree(&self, path: &str) -> AppResult<()> {
        crate::fs::guard(path)?;
        if !self.is_real_dir(path).await? {
            return self.remove_file(path).await;
        }
        // Post-order walk without recursion: a dir is removed after its contents.
        let mut stack = vec![(path.to_string(), false)];
        while let Some((dir, emptied)) = stack.pop() {
            if emptied {
                self.remove_dir(&dir).await?;
                continue;
            }
            stack.push((dir.clone(), true));
            for entry in self.list(&dir).await? {
                if entry.is_dir && !entry.is_symlink {
                    stack.push((entry.path, false));
                } else {
                    self.remove_file(&entry.path).await?;
                }
            }
        }
        Ok(())
    }

    /// Remove empty directories from `dir` up to and including `stop`.
    pub async fn rmdir_up(&self, dir: &str, stop: &str) {
        let mut current = dir.to_string();
        while current.starts_with(stop) {
            if self.remove_dir(&current).await.is_err() || current == stop {
                break;
            }
            current = crate::backup::parent(&current).to_string();
        }
    }

    pub async fn close(&self) {
        match self {
            RemoteFs::Ftp(ftp) => ftp.quit().await,
            RemoteFs::Sftp(sftp) => {
                let _ = sftp.close().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_plain_name, Kept};
    use russh_sftp::protocol::FileAttributes;

    #[test]
    fn kept_drops_file_type_bits_and_owner_on_demand() {
        let mut attrs = FileAttributes::empty();
        attrs.permissions = Some(0o100755);
        attrs.uid = Some(1000);
        attrs.gid = Some(33);
        let kept = Kept::from_sftp(&attrs).unwrap();
        assert_eq!(kept.attrs(true).permissions, Some(0o755));
        assert_eq!(kept.attrs(true).uid, Some(1000));
        assert_eq!(kept.attrs(false).permissions, Some(0o755));
        assert_eq!((kept.attrs(false).uid, kept.attrs(false).gid), (None, None));
        assert!(Kept::from_sftp(&FileAttributes::empty()).is_none());
    }

    #[test]
    fn plain_names_only() {
        assert!(is_plain_name("index.php") && is_plain_name(".env") && is_plain_name("..x"));
        for bad in ["", ".", "..", "../x", "/etc"] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
    }
}
