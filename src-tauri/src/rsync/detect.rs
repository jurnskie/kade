//! Which rsync runs here and on the server, and whether the pair can sync.

use std::path::{Path, PathBuf};
use std::time::Duration;

use russh::ChannelMsg;
use serde::Serialize;

use crate::error::AppResult;
use crate::ssh::Session;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// One line on the server: where rsync is, and its version banner.
const REMOTE_PROBE: &str = "command -v rsync && rsync --version 2>/dev/null | head -n 1";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Implementation {
    /// rsync.samba.org: GNU 3.x, or 2.6.9 on older macOS.
    Rsync,
    /// The BSD rewrite that macOS 15.4+ ships as `/usr/bin/rsync`.
    Openrsync,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Version {
    pub implementation: Implementation,
    /// `3.2.7`; `None` for openrsync, which only reports its protocol.
    pub version: Option<String>,
    pub protocol: u32,
}

impl Version {
    fn numbers(&self) -> (u32, u32, u32) {
        let mut parts = self.version.as_deref().unwrap_or("").split('.').map(|p| {
            // "3.4.1pre1" counts as 3.4.1.
            p.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)
        });
        (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0))
    }

    /// GNU ≥ 3.0, the old 2.6.9, or any openrsync.
    pub fn supported(&self) -> bool {
        match self.implementation {
            Implementation::Openrsync => true,
            Implementation::Rsync => self.numbers() >= (2, 6, 9),
        }
    }

    pub fn is_gnu3(&self) -> bool {
        self.implementation == Implementation::Rsync && self.numbers() >= (3, 0, 0)
    }

    /// Clients before 3.4.0 let a malicious server write outside the
    /// destination (CVE-2024-12087/12088), which matters when pulling.
    fn pull_is_risky(&self) -> bool {
        self.implementation == Implementation::Rsync && self.numbers() < (3, 4, 0)
    }
}

/// The first line of `rsync --version`.
pub fn parse_version(line: &str) -> Option<Version> {
    let words: Vec<&str> = line.split_whitespace().collect();
    // "… protocol version 31": always the last word.
    let protocol = match words.as_slice() {
        [.., "protocol", "version", n] => n.parse().ok(),
        _ => None,
    };
    if words.first() == Some(&"openrsync:") {
        return Some(Version { implementation: Implementation::Openrsync, version: None, protocol: protocol? });
    }
    if words.first() != Some(&"rsync") {
        return None;
    }
    let version = words.windows(2).find(|w| w[0] == "version").map(|w| w[1].to_string())?;
    version.starts_with(|c: char| c.is_ascii_digit()).then_some(())?;
    Some(Version { implementation: Implementation::Rsync, version: Some(version), protocol: protocol? })
}

/// What the frontend shows: the menu items are disabled with `reason` as a tooltip.
#[derive(Debug, Clone, Serialize)]
pub struct RsyncSupport {
    pub available: bool,
    pub reason: Option<String>,
    pub local: Option<Version>,
    pub remote: Option<Version>,
    /// Shown before a "Sync from server": the local rsync is too old to trust a hostile server.
    pub pull_warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LocalRsync {
    pub path: PathBuf,
    pub version: Version,
}

/// Everything a sync on this session needs; cached per session.
#[derive(Debug, Clone)]
pub struct Detected {
    pub support: RsyncSupport,
    pub local: Option<LocalRsync>,
    /// The server's rsync, as `command -v` printed it.
    pub remote_path: Option<String>,
    pub remote: Option<Version>,
}

impl Detected {
    fn unavailable(reason: String, local: Option<LocalRsync>) -> Self {
        Detected {
            support: RsyncSupport {
                available: false,
                reason: Some(reason),
                local: local.as_ref().map(|l| l.version.clone()),
                remote: None,
                pull_warning: None,
            },
            local,
            remote_path: None,
            remote: None,
        }
    }
}

async fn version_of(path: &Path) -> Option<Version> {
    let mut cmd = crate::hostenv::async_command(path);
    cmd.arg("--version").kill_on_drop(true).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await.ok()?.ok()?;
    parse_version(String::from_utf8_lossy(&out.stdout).lines().next()?)
}

/// The local rsync to use. A GUI launch has a minimal PATH, so the usual
/// places are tried first; GNU 3.x wins over openrsync when both exist.
pub async fn local() -> Option<LocalRsync> {
    if let Some(path) = std::env::var_os("KADE_RSYNC") {
        let path = PathBuf::from(path);
        return version_of(&path).await.map(|version| LocalRsync { path, version });
    }
    let mut candidates: Vec<PathBuf> = ["/opt/homebrew/bin/rsync", "/usr/local/bin/rsync", "/usr/bin/rsync"].map(PathBuf::from).into();
    let which = crate::hostenv::async_command("which").arg("rsync").kill_on_drop(true).output();
    if let Ok(Ok(out)) = tokio::time::timeout(PROBE_TIMEOUT, which).await {
        let found = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if found.starts_with('/') {
            candidates.push(PathBuf::from(found));
        }
    }
    let mut best: Option<LocalRsync> = None;
    for path in candidates {
        if !path.is_file() || best.as_ref().is_some_and(|b| b.path == path) {
            continue;
        }
        let Some(version) = version_of(&path).await.filter(Version::supported) else { continue };
        if best.as_ref().is_none_or(|b| version.is_gnu3() && !b.version.is_gnu3()) {
            best = Some(LocalRsync { path, version });
        }
    }
    best
}

/// Outcome of the server probe.
enum Remote {
    Found { path: String, version: Version },
    Missing,
    ExecRefused,
}

async fn probe_remote(session: &Session) -> AppResult<Remote> {
    let mut channel = session.ssh()?.channel_open_session().await?;
    channel.exec(true, REMOTE_PROBE).await?;
    let mut out = Vec::new();
    let mut status = None;
    let read = async {
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                // ForceCommand internal-sftp, a restricted shell: no exec at all.
                ChannelMsg::Failure => return true,
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        false
    };
    let refused = tokio::time::timeout(PROBE_TIMEOUT, read).await.unwrap_or(true);
    if refused {
        return Ok(Remote::ExecRefused);
    }
    let text = String::from_utf8_lossy(&out);
    let mut lines = text.lines();
    let path = lines.next().unwrap_or("").trim().to_string();
    match (status, lines.next().and_then(parse_version)) {
        (Some(0), Some(version)) if !path.is_empty() => Ok(Remote::Found { path, version }),
        _ => Ok(Remote::Missing),
    }
}

pub async fn detect(session: &Session) -> Detected {
    if session.ssh.is_none() {
        return Detected::unavailable(tr!("Sync needs an SSH connection", "Synchroniseren kan alleen via SSH"), None);
    }
    if session.fs.is_none() || session.remote_home.is_none() {
        return Detected::unavailable(tr!("Sync needs SFTP on the server", "Synchroniseren heeft SFTP op de server nodig"), None);
    }
    let Some(local) = local().await else {
        return Detected::unavailable(
            tr!("Install rsync (e.g. `brew install rsync`)", "Installeer rsync (bijv. `brew install rsync`)"),
            None,
        );
    };
    let (path, version) = match probe_remote(session).await {
        Ok(Remote::Found { path, version }) if version.supported() => (path, version),
        Ok(Remote::Found { version, .. }) => {
            let v = version.version.unwrap_or_default();
            return Detected::unavailable(
                tr!("rsync {v} on the server is too old", "rsync {v} op de server is te oud", v = v),
                Some(local),
            );
        }
        Ok(Remote::Missing) => {
            return Detected::unavailable(tr!("rsync isn't installed on the server", "rsync staat niet op de server"), Some(local))
        }
        Ok(Remote::ExecRefused) => {
            return Detected::unavailable(tr!("The server only allows SFTP", "De server staat alleen SFTP toe"), Some(local))
        }
        Err(e) => return Detected::unavailable(e.to_string(), Some(local)),
    };
    let pull_warning = local.version.pull_is_risky().then(|| {
        tr!(
            "Your rsync is older than 3.4.0: a malicious server could write outside the destination folder. Update rsync (e.g. `brew install rsync`).",
            "Je rsync is ouder dan 3.4.0: een kwaadwillende server kan buiten de doelmap schrijven. Werk rsync bij (bijv. `brew install rsync`)."
        )
    });
    Detected {
        support: RsyncSupport {
            available: true,
            reason: None,
            local: Some(local.version.clone()),
            remote: Some(version.clone()),
            pull_warning,
        },
        local: Some(local),
        remote_path: Some(path),
        remote: Some(version),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_banners() {
        let v = parse_version("openrsync: protocol version 29").unwrap();
        assert_eq!((v.implementation, v.version.as_deref(), v.protocol), (Implementation::Openrsync, None, 29));
        assert!(v.supported() && !v.is_gnu3() && !v.pull_is_risky());

        let v = parse_version("rsync  version 2.6.9  protocol version 29").unwrap();
        assert_eq!((v.implementation, v.version.as_deref(), v.protocol), (Implementation::Rsync, Some("2.6.9"), 29));
        assert!(v.supported() && !v.is_gnu3() && v.pull_is_risky());

        let v = parse_version("rsync  version 3.2.7  protocol version 31").unwrap();
        assert_eq!((v.version.as_deref(), v.protocol), (Some("3.2.7"), 31));
        assert!(v.supported() && v.is_gnu3() && v.pull_is_risky());

        let v = parse_version("rsync  version v3.4.1  protocol version 32");
        assert!(v.is_none(), "a version must start with a digit");
        let v = parse_version("rsync  version 3.4.1  protocol version 32").unwrap();
        assert!(v.is_gnu3() && !v.pull_is_risky());

        assert!(!parse_version("rsync  version 2.6.3  protocol version 28").unwrap().supported());
        for garbage in ["", "zsh: command not found: rsync", "rsync version", "openrsync: protocol version x", "rsync  version 3.2.7"] {
            assert!(parse_version(garbage).is_none(), "{garbage:?}");
        }
    }
}
