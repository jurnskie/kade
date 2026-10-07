//! FTP and FTPS (explicit TLS on 21, implicit TLS on 990).
//!
//! An FTP control connection does one thing at a time, so every operation
//! takes the connection lock; a running transfer keeps holding it until its
//! data stream is finished. Concurrent jobs simply wait their turn.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use suppaftp::list::File as ListFile;
use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsFtpStream, AsyncRustlsStream, TransferStream};
use suppaftp::types::FileType;
use suppaftp::{FtpError, Status};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::error::{AppError, AppResult};
use crate::fs::Entry;
use crate::profiles::{Protocol, ServerProfile};
use crate::remote::Meta;
use crate::ssh::join_remote;

const KEEPALIVE: Duration = Duration::from_secs(60);

impl From<FtpError> for AppError {
    fn from(e: FtpError) -> Self {
        AppError::other(e)
    }
}

/// The server's "no such file or directory" replies. 450 is what some
/// servers (ProFTPD) send for a LIST of a folder that doesn't exist.
fn is_missing(e: &FtpError) -> bool {
    matches!(e, FtpError::UnexpectedResponse(r) if matches!(r.status, Status::FileUnavailable | Status::RequestFileActionIgnored))
}

pub struct Ftp {
    conn: Arc<Mutex<AsyncRustlsFtpStream>>,
    /// Server understands MLSD/MLST (exact sizes, dates and types).
    mlsd: bool,
    /// Server understands MFMT (set modification time).
    mfmt: bool,
}

fn tls_connector(tls12_only: bool) -> AppResult<AsyncRustlsConnector> {
    let mut roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // Extra trusted CAs (PEM), e.g. a homelab's own CA: KADE_EXTRA_CA=/path/ca.pem
    if let Ok(path) = std::env::var("KADE_EXTRA_CA") {
        let pem = std::fs::read(&path).map_err(|e| AppError::other(format!("KADE_EXTRA_CA: {e}")))?;
        for cert in rustls_pemfile::certs(&mut pem.as_slice()) {
            roots.add(cert.map_err(AppError::other)?).map_err(AppError::other)?;
        }
    }
    let versions: &[&rustls::SupportedProtocolVersion] = if tls12_only { &[&rustls::version::TLS12] } else { rustls::DEFAULT_VERSIONS };
    // The default config keeps a session cache, which FTPS data connections
    // need: many servers require them to resume the control connection's session.
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(versions)
        .map_err(AppError::other)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(AsyncRustlsConnector::from(tokio_rustls::TlsConnector::from(Arc::new(config))))
}

/// Connect and log in. Returns the connection and the login directory.
///
/// FTPS prefers TLS 1.2: servers like vsftpd and FileZilla Server require every
/// data connection to resume the control connection's TLS session ("522 session
/// reuse required"). TLS 1.3 tickets are single-use, so that breaks after a few
/// transfers; TLS 1.2 session resumption does not. Servers that only speak
/// TLS 1.3 get a second attempt with it.
pub async fn connect(profile: &ServerProfile, password: &str) -> AppResult<(Ftp, String)> {
    // Don't let an unreachable host hang for minutes; explain network errors.
    let attempt = tokio::time::timeout(std::time::Duration::from_secs(20), connect_inner(profile, password)).await;
    match attempt {
        Ok(Err(AppError::Other { message })) if !message.contains("TLS") => {
            Err(crate::error::network_error(&profile.host, profile.port, message))
        }
        Ok(result) => result,
        Err(_) => Err(crate::error::network_error(&profile.host, profile.port, "timed out")),
    }
}

async fn connect_inner(profile: &ServerProfile, password: &str) -> AppResult<(Ftp, String)> {
    match connect_with(profile, password, true).await {
        Err(e) if profile.protocol == Protocol::Ftps && e.to_string().contains("TLS") => connect_with(profile, password, false).await,
        other => other,
    }
}

async fn connect_with(profile: &ServerProfile, password: &str, tls12_only: bool) -> AppResult<(Ftp, String)> {
    let addr = (profile.host.as_str(), profile.port);
    let mut conn = match profile.protocol {
        Protocol::Ftps if profile.port == 990 => {
            AsyncRustlsFtpStream::connect_secure_implicit(addr, tls_connector(tls12_only)?, &profile.host)
                .await
                .map_err(|e| AppError::other(tr!("TLS connection failed: {e}", "TLS-verbinding mislukt: {e}", e = e)))?
        }
        Protocol::Ftps => {
            let plain = AsyncRustlsFtpStream::connect(addr).await?;
            plain
                .into_secure(tls_connector(tls12_only)?, &profile.host)
                .await
                .map_err(|e| AppError::other(tr!("TLS connection failed: {e}", "TLS-verbinding mislukt: {e}", e = e)))?
        }
        _ => AsyncRustlsFtpStream::connect(addr).await?,
    };

    if let Err(e) = conn.login(profile.user.as_str(), password).await {
        return Err(AppError::AuthFailed { message: e.to_string() });
    }
    conn.transfer_type(FileType::Binary).await?;
    let features = conn.feat().await.unwrap_or_default();
    let has = |f: &str| features.keys().any(|k| k.eq_ignore_ascii_case(f));
    if has("UTF8") {
        let _ = conn.custom_command("OPTS UTF8 ON", &[Status::CommandOk]).await;
    }
    let home = conn.pwd().await.unwrap_or_else(|_| "/".into());

    let ftp = Ftp { conn: Arc::new(Mutex::new(conn)), mlsd: has("MLST"), mfmt: has("MFMT") };
    spawn_keepalive(Arc::downgrade(&ftp.conn));
    Ok((ftp, home))
}

/// Servers drop idle control connections; a NOOP now and then keeps it open.
fn spawn_keepalive(conn: Weak<Mutex<AsyncRustlsFtpStream>>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(KEEPALIVE).await;
            let Some(arc) = conn.upgrade() else { break };
            // Skip while a transfer holds the connection; that keeps it alive anyway.
            let guard = arc.try_lock();
            if let Ok(mut c) = guard {
                let _ = c.noop().await;
            };
        }
    });
}

fn unix(t: SystemTime) -> Option<i64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

fn permissions(f: &ListFile) -> Option<String> {
    use suppaftp::list::PosixPexQuery::{Group, Others, Owner};
    if f.uid().is_none() && f.gid().is_none() && !f.can_read(Owner) {
        return None; // DOS-style listings carry no permissions
    }
    let kind = if f.is_directory() {
        'd'
    } else if f.is_symlink() {
        'l'
    } else {
        '-'
    };
    let mut s = String::from(kind);
    for who in [Owner, Group, Others] {
        s.push(if f.can_read(who) { 'r' } else { '-' });
        s.push(if f.can_write(who) { 'w' } else { '-' });
        s.push(if f.can_execute(who) { 'x' } else { '-' });
    }
    Some(s)
}

impl Ftp {
    async fn lock(&self) -> tokio::sync::MutexGuard<'_, AsyncRustlsFtpStream> {
        self.conn.lock().await
    }

    async fn raw_list(&self, dir: &str) -> Result<Vec<ListFile>, FtpError> {
        let mut c = self.lock().await;
        let files = if self.mlsd {
            c.mlsd(Some(dir)).await?.iter().filter_map(|l| suppaftp::list::ListParser::parse_mlsd(l).ok()).collect()
        } else {
            c.list(Some(dir)).await?.iter().filter_map(|l| ListFile::try_from(l.as_str()).ok()).collect()
        };
        Ok(files)
    }

    /// Whether `path` is a directory, by trying to change into it.
    async fn is_dir(&self, path: &str) -> bool {
        let mut c = self.lock().await;
        let Ok(back) = c.pwd().await else { return false };
        let ok = c.cwd(path).await.is_ok();
        if ok {
            let _ = c.cwd(back).await;
        }
        ok
    }

    pub async fn list(&self, dir: &str) -> AppResult<Vec<Entry>> {
        let mut out = Vec::new();
        for f in self.raw_list(dir).await? {
            let name = f.name().to_string();
            if name == "." || name == ".." || name.is_empty() {
                continue;
            }
            let path = join_remote(dir, &name);
            let is_dir = if f.is_symlink() { self.is_dir(&path).await } else { f.is_directory() };
            out.push(Entry {
                is_dir,
                is_symlink: f.is_symlink(),
                size: f.size() as u64,
                modified: unix(f.modified()),
                permissions: permissions(&f),
                name,
                path,
            });
        }
        Ok(out)
    }

    /// Look the item up in its parent's listing: works on every server,
    /// unlike MLST/SIZE/MDTM which are optional.
    pub async fn stat(&self, path: &str) -> AppResult<Option<Meta>> {
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() {
            return Ok(Some(Meta { is_dir: true, is_symlink: false, size: 0, mtime: None }));
        }
        let (parent, name) = match trimmed.rfind('/') {
            Some(0) => ("/", &trimmed[1..]),
            Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
            None => (".", trimmed),
        };
        let files = match self.raw_list(parent).await {
            Ok(files) => files,
            Err(e) if is_missing(&e) => return Ok(None),
            // Anything else (a dropped connection, a timeout) must not read as "absent":
            // an overwrite would then skip the backup.
            Err(e) => return Err(e.into()),
        };
        let Some(f) = files.into_iter().find(|f| f.name() == name) else { return Ok(None) };
        let is_dir = if f.is_symlink() { self.is_dir(path).await } else { f.is_directory() };
        Ok(Some(Meta { is_dir, is_symlink: f.is_symlink(), size: f.size() as u64, mtime: unix(f.modified()) }))
    }

    pub async fn mkdir(&self, path: &str) -> AppResult<()> {
        Ok(self.lock().await.mkdir(path).await?)
    }

    pub async fn remove_file(&self, path: &str) -> AppResult<()> {
        Ok(self.lock().await.rm(path).await?)
    }

    pub async fn remove_dir(&self, path: &str) -> AppResult<()> {
        Ok(self.lock().await.rmdir(path).await?)
    }

    pub async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        Ok(self.lock().await.rename(from, to).await?)
    }

    /// Best effort. MFMT is the standard; vsftpd and some others accept the
    /// older two-argument `MDTM <time> <path>` instead.
    pub async fn set_mtime(&self, path: &str, unix_secs: i64) {
        let Some(t) = chrono::DateTime::from_timestamp(unix_secs, 0) else { return };
        let stamp = t.format("%Y%m%d%H%M%S");
        let cmd = if self.mfmt { format!("MFMT {stamp} {path}") } else { format!("MDTM {stamp} {path}") };
        let _ = self.lock().await.custom_command(cmd, &[Status::File]).await;
    }

    /// LIST shows times to the minute at best; MLSD to the second.
    pub fn mtime_tolerance(&self) -> i64 {
        if self.mlsd {
            0
        } else {
            60
        }
    }

    /// Open a download; the connection stays locked until `finish`.
    pub async fn reader(&self, path: &str) -> AppResult<FtpTransfer> {
        let mut guard = self.conn.clone().lock_owned().await;
        let stream = guard.retr_as_stream(path).await?;
        Ok(FtpTransfer { stream, _guard: guard })
    }

    /// Open an upload (creates or truncates); locked until `finish`.
    pub async fn writer(&self, path: &str) -> AppResult<FtpTransfer> {
        let mut guard = self.conn.clone().lock_owned().await;
        let stream = guard.put_with_stream(path).await?;
        Ok(FtpTransfer { stream, _guard: guard })
    }

    pub async fn quit(&self) {
        let _ = self.lock().await.quit().await;
    }
}

/// An open FTP data transfer plus the connection lock it needs.
pub struct FtpTransfer {
    stream: TransferStream<AsyncRustlsStream>,
    _guard: OwnedMutexGuard<AsyncRustlsFtpStream>,
}

impl AsyncRead for FtpTransfer {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for FtpTransfer {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl FtpTransfer {
    /// Completes the transfer and reads the server's verdict.
    pub async fn finish(mut self) -> AppResult<()> {
        self.stream.flush().await?;
        Ok(self.stream.finish().await?)
    }
}
