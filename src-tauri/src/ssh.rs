use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::agent::client::AgentClient;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use serde::Serialize;

use crate::error::{network_error, AppError, AppResult};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
use crate::fs::Entry;
use crate::profiles::{Auth, Protocol, ServerProfile};
use crate::remote::RemoteFs;
use crate::store::Workspace;

pub struct Client {
    host: String,
    port: u16,
    /// Fingerprint the user explicitly agreed to trust for an unknown host.
    accept_fingerprint: Option<String>,
    /// Why the host key was rejected, so `connect` can report something useful
    /// instead of russh's generic UnknownKey.
    rejection: Arc<Mutex<Option<AppError>>>,
}

/// `~/.ssh/known_hosts`, or `$KADE_KNOWN_HOSTS` (used by tests).
fn known_hosts_path() -> AppResult<PathBuf> {
    if let Ok(p) = std::env::var("KADE_KNOWN_HOSTS") {
        return Ok(PathBuf::from(p));
    }
    Ok(dirs::home_dir().ok_or_else(|| AppError::other(tr!("No home directory", "Geen home-map")))?.join(".ssh/known_hosts"))
}

pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = key.public_key();
        let fp = fingerprint(&key);
        let known_hosts = known_hosts_path().map_err(|e| russh::Error::IO(std::io::Error::other(e.to_string())))?;
        let verdict = match russh::keys::check_known_hosts_path(&self.host, self.port, &key, &known_hosts) {
            Ok(true) => Ok(()),
            Ok(false) if self.accept_fingerprint.as_deref() == Some(fp.as_str()) => {
                russh::keys::known_hosts::learn_known_hosts_path(&self.host, self.port, &key, &known_hosts).map_err(AppError::other)
            }
            Ok(false) => Err(AppError::HostKeyUnknown { fingerprint: fp, algorithm: key.algorithm().to_string() }),
            Err(russh::keys::Error::KeyChanged { line }) => Err(AppError::HostKeyChanged { line }),
            Err(e) => Err(AppError::other(e)),
        };
        match verdict {
            Ok(()) => Ok(true),
            Err(e) => {
                *self.rejection.lock().unwrap() = Some(e);
                Ok(false)
            }
        }
    }
}

/// One open connection. SFTP/SSH servers have an SSH handle (terminal,
/// tunnels); FTP servers only have a file system.
pub struct Session {
    pub ssh: Option<Handle<Client>>,
    /// Absent for SSH-only profiles whose server has no SFTP subsystem.
    pub fs: Option<RemoteFs>,
    pub server_id: String,
    pub server_name: String,
    /// The login user's home directory on the server (where backups go).
    pub remote_home: Option<String>,
}

impl Session {
    pub fn fs(&self) -> AppResult<&RemoteFs> {
        self.fs.as_ref().ok_or_else(|| AppError::Unsupported {
            message: tr!("This server doesn't offer file transfer", "Deze server biedt geen bestandsoverdracht aan"),
        })
    }

    pub fn ssh(&self) -> AppResult<&Handle<Client>> {
        self.ssh.as_ref().ok_or_else(|| AppError::Unsupported { message: tr!("A terminal needs SSH", "Een terminal kan alleen via SSH") })
    }
}

#[derive(Serialize)]
pub struct Connected {
    pub session_id: String,
    pub home: String,
    pub auth_label: String,
    pub has_files: bool,
    pub has_terminal: bool,
}

/// Where the 1Password SSH agent listens on this platform.
fn one_password_socket() -> AppResult<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| AppError::other(tr!("No home directory", "Geen home-map")))?;
    #[cfg(target_os = "macos")]
    let path = home.join("Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock");
    #[cfg(not(target_os = "macos"))]
    let path = home.join(".1password/agent.sock");
    Ok(path)
}

fn agent_socket(auth: &Auth) -> AppResult<PathBuf> {
    match auth {
        Auth::OnePassword { .. } => one_password_socket(),
        _ => std::env::var("SSH_AUTH_SOCK")
            .map(PathBuf::from)
            .map_err(|_| AppError::AgentUnavailable { message: tr!("SSH_AUTH_SOCK is not set", "SSH_AUTH_SOCK is niet gezet") }),
    }
}

async fn open_agent(auth: &Auth) -> AppResult<AgentClient<tokio::net::UnixStream>> {
    let sock = agent_socket(auth)?;
    AgentClient::connect_uds(&sock).await.map_err(|e| AppError::AgentUnavailable { message: format!("{} ({e})", sock.display()) })
}

#[derive(Serialize)]
pub struct AgentKey {
    pub comment: String,
    pub fingerprint: String,
    pub algorithm: String,
}

pub async fn agent_keys(auth: &Auth) -> AppResult<Vec<AgentKey>> {
    let mut agent = open_agent(auth).await?;
    let ids = agent.request_identities().await.map_err(AppError::other)?;
    Ok(ids
        .iter()
        .map(|id| {
            let key = id.public_key();
            AgentKey { comment: id.comment().to_string(), fingerprint: fingerprint(&key), algorithm: key.algorithm().to_string() }
        })
        .collect())
}

fn expand_tilde(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// Log in with a key from an SSH agent. Only keys whose fingerprint is in
/// `allowed` are offered (all of them when `None`): every offered key counts
/// as an attempt against the server's MaxAuthTries. `Ok(None)` means the agent
/// had no usable key (or the pinned one is missing), so another route may be tried.
async fn auth_with_agent(
    handle: &mut Handle<Client>,
    user: &str,
    auth: &Auth,
    allowed: Option<&[String]>,
    label: &str,
) -> AppResult<Option<String>> {
    let mut agent = open_agent(auth).await?;
    let ids = agent.request_identities().await.map_err(AppError::other)?;
    let candidates: Vec<_> = ids.into_iter().filter(|id| allowed.is_none_or(|a| a.contains(&fingerprint(&id.public_key())))).collect();
    if candidates.is_empty() {
        return Ok(None);
    }
    let rsa_hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
    for id in candidates {
        let key = id.public_key().into_owned();
        let result = handle
            .authenticate_publickey_with(user, key, rsa_hash, &mut agent)
            .await
            .map_err(|e| AppError::AuthFailed { message: e.to_string() })?;
        if result.success() {
            return Ok(Some(format!("{label} · {}", id.comment())));
        }
    }
    Err(AppError::AuthFailed { message: tr!("the server rejected all keys", "server weigerde alle sleutels") })
}

/// The password to use: typed in by the user, or read from 1Password.
async fn resolve_password(auth: &Auth, typed: Option<String>) -> AppResult<String> {
    match auth {
        Auth::OnePasswordSecret { reference, account } => crate::onepassword::read(reference, account.as_deref()).await,
        _ => typed.ok_or(AppError::PasswordRequired),
    }
}

async fn authenticate(
    handle: &mut Handle<Client>,
    profile: &ServerProfile,
    password: Option<String>,
    default_vault: Option<&str>,
) -> AppResult<String> {
    let user = profile.user.clone();
    match &profile.auth {
        Auth::Agent { key_fingerprint } => {
            let pin = key_fingerprint.as_ref().map(std::slice::from_ref);
            match auth_with_agent(handle, &user, &profile.auth, pin, "ssh-agent").await? {
                Some(label) => Ok(label),
                None => Err(AppError::AuthFailed {
                    message: match key_fingerprint {
                        Some(_) => {
                            tr!("the chosen key is not (or no longer) in the agent", "de gekozen sleutel staat niet (meer) in de agent")
                        }
                        None => tr!("the agent has no keys", "de agent heeft geen sleutels"),
                    },
                }),
            }
        }
        Auth::OnePassword { key_fingerprint, account, key_item } => {
            // Without a key of its own, the workspace's default vault limits which keys are tried.
            let vault_keys = match default_vault {
                Some(vault) if key_fingerprint.is_none() && key_item.is_none() => {
                    Some(crate::onepassword::ssh_keys(account.as_deref(), Some(vault)).await?)
                }
                _ => None,
            };
            let mut key_fingerprint = key_fingerprint.clone();
            let mut key_item = key_item.clone();
            let allowed: Option<Vec<String>> = match &vault_keys {
                Some(keys) => Some(keys.iter().map(|k| k.fingerprint.clone()).filter(|f| !f.is_empty()).collect()),
                None => key_fingerprint.clone().map(|f| vec![f]),
            };
            // A lone key in the vault is unambiguous, so it can be fetched when the agent lacks it.
            if let Some([only]) = vault_keys.as_deref() {
                key_fingerprint = Some(only.fingerprint.clone());
                key_item = Some(only.item.clone());
            }
            // The agent first: no extra prompt when this machine offers the key.
            let via_agent = auth_with_agent(handle, &user, &profile.auth, allowed.as_deref(), "1Password").await;
            match (via_agent, &key_item) {
                (Ok(Some(label)), _) => Ok(label),
                // Not offered here (or no agent): fetch it from the chosen account/vault.
                (Ok(None) | Err(AppError::AgentUnavailable { .. }), Some(item)) => {
                    let pem = crate::onepassword::read_ssh_key(item, account.as_deref()).await?;
                    let key = russh::keys::decode_secret_key(&pem, None).map_err(|e| AppError::AuthFailed {
                        message: tr!("key from 1Password unreadable: {e}", "sleutel uit 1Password onleesbaar: {e}", e = e),
                    })?;
                    if let Some(pin) = &key_fingerprint {
                        if fingerprint(key.public_key()) != *pin {
                            return Err(AppError::AuthFailed {
                                message: tr!(
                                    "the key in 1Password has a different fingerprint than saved",
                                    "de sleutel in 1Password heeft een andere vingerafdruk dan opgeslagen"
                                ),
                            });
                        }
                    }
                    let rsa_hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
                    let result = handle.authenticate_publickey(&user, PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash)).await?;
                    if result.success() {
                        Ok(tr!("1Password · key from vault", "1Password · sleutel uit kluis"))
                    } else {
                        Err(AppError::AuthFailed { message: tr!("the server rejected the key", "server weigerde de sleutel") })
                    }
                }
                (Ok(None) | Err(AppError::AgentUnavailable { .. }), None) if vault_keys.is_some() => Err(AppError::AuthFailed {
                    message: tr!(
                        "the 1Password vault has no single key to use. Choose a key in the connection, or a default key for the workspace.",
                        "de 1Password-kluis heeft geen eenduidige sleutel. Kies een sleutel in de verbinding, of een standaardsleutel voor de workspace."
                    ),
                }),
                (Ok(None), None) => Err(AppError::AuthFailed {
                    message: tr!(
                        "the 1Password SSH agent on this computer doesn't offer this key. Choose the 1Password \
                         account and vault in the connection, and Kade fetches the key itself.",
                        "de 1Password SSH-agent op deze computer biedt deze sleutel niet aan. Kies in de \
                         verbinding het 1Password-account en de kluis, dan haalt Kade hem zelf op."
                    ),
                }),
                (Err(e), _) => Err(e),
            }
        }
        Auth::KeyFile { path } => {
            let full = expand_tilde(path);
            let key = match russh::keys::load_secret_key(&full, password.as_deref()) {
                Ok(k) => k,
                Err(_) if password.is_none() => return Err(AppError::PasswordRequired),
                Err(e) => return Err(AppError::AuthFailed { message: e.to_string() }),
            };
            let rsa_hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
            let result = handle.authenticate_publickey(&user, PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash)).await?;
            if result.success() {
                Ok(tr!("key · {path}", "sleutel · {path}", path = path))
            } else {
                Err(AppError::AuthFailed { message: tr!("the server rejected the key", "server weigerde de sleutel") })
            }
        }
        Auth::Password | Auth::OnePasswordSecret { .. } => {
            let password = resolve_password(&profile.auth, password).await?;
            if handle.authenticate_password(&user, password).await?.success() {
                Ok(match profile.auth {
                    Auth::OnePasswordSecret { .. } => tr!("1Password · password", "1Password · wachtwoord"),
                    _ => tr!("password", "wachtwoord"),
                })
            } else {
                Err(AppError::AuthFailed { message: tr!("wrong password", "onjuist wachtwoord") })
            }
        }
    }
}

/// The 1Password defaults of a workspace, applied to a connection without its own.
/// Returns the profile and, when only a default vault is set, that vault's id.
/// A connection's own account and key always win.
fn apply_workspace_defaults(profile: &ServerProfile, ws: Option<&Workspace>) -> (ServerProfile, Option<String>) {
    let mut p = profile.clone();
    let Some(ws) = ws else {
        return (p, None);
    };
    let mut vault = None;
    match &mut p.auth {
        Auth::OnePassword { account, key_fingerprint, key_item } => {
            if account.is_none() {
                account.clone_from(&ws.op_account);
            }
            // The workspace's vault and key belong to its account; without one
            // they were picked from 1Password's default account.
            let same_account = ws.op_account.is_none() || *account == ws.op_account;
            if same_account && key_fingerprint.is_none() && key_item.is_none() {
                match (&ws.op_key_fingerprint, &ws.op_vault) {
                    (Some(fp), _) => {
                        *key_fingerprint = Some(fp.clone());
                        key_item.clone_from(&ws.op_key_item);
                    }
                    (None, Some(v)) => vault = Some(v.clone()),
                    (None, None) => {}
                }
            }
        }
        Auth::OnePasswordSecret { account, .. } if account.is_none() => account.clone_from(&ws.op_account),
        _ => {}
    }
    (p, vault)
}

fn with_workspace_defaults(profile: &ServerProfile) -> (ServerProfile, Option<String>) {
    let data = crate::store::load().ok();
    apply_workspace_defaults(profile, data.as_ref().and_then(|d| d.workspace_of(profile)).as_ref())
}

pub async fn connect(
    profile: &ServerProfile,
    password: Option<String>,
    accept_fingerprint: Option<String>,
) -> AppResult<(Session, Connected)> {
    let (profile, default_vault) = with_workspace_defaults(profile);
    let profile = &profile;
    let (session, home, auth_label) = match profile.protocol {
        Protocol::Ftp | Protocol::Ftps => connect_ftp(profile, password).await?,
        Protocol::Sftp | Protocol::Ssh => connect_ssh(profile, password, accept_fingerprint, default_vault.as_deref()).await?,
    };
    let home = match &profile.remote_path {
        Some(p) if !p.is_empty() => p.clone(),
        _ => home,
    };
    let connected = Connected {
        session_id: uuid::Uuid::new_v4().to_string(),
        home,
        auth_label,
        has_files: session.fs.is_some(),
        has_terminal: session.ssh.is_some(),
    };
    Ok((session, connected))
}

async fn connect_ftp(profile: &ServerProfile, password: Option<String>) -> AppResult<(Session, String, String)> {
    if matches!(profile.auth, Auth::OnePassword { .. } | Auth::Agent { .. } | Auth::KeyFile { .. }) {
        return Err(AppError::Unsupported {
            message: tr!("FTP has no SSH keys; choose a password", "FTP kent geen SSH-sleutels; kies een wachtwoord"),
        });
    }
    let password = resolve_password(&profile.auth, password).await?;
    let (ftp, home) = crate::ftp::connect(profile, &password).await?;
    let source = match profile.auth {
        Auth::OnePasswordSecret { .. } => "1Password".to_string(),
        _ => tr!("password", "wachtwoord"),
    };
    let label = match profile.protocol {
        Protocol::Ftps => format!("{source} · FTPS (TLS)"),
        _ => tr!("{source} · FTP unencrypted", "{source} · FTP onversleuteld", source = source),
    };
    let session = Session {
        ssh: None,
        fs: Some(RemoteFs::Ftp(ftp)),
        server_id: profile.id.clone(),
        server_name: profile.name.clone(),
        remote_home: Some(home.clone()),
    };
    Ok((session, home, label))
}

async fn connect_ssh(
    profile: &ServerProfile,
    password: Option<String>,
    accept_fingerprint: Option<String>,
    default_vault: Option<&str>,
) -> AppResult<(Session, String, String)> {
    // nodelay: SFTP is request/response; Nagle would hold back small requests.
    let config = Arc::new(client::Config { keepalive_interval: Some(Duration::from_secs(30)), nodelay: true, ..Default::default() });
    let rejection = Arc::new(Mutex::new(None));
    let handler = Client { host: profile.host.clone(), port: profile.port, accept_fingerprint, rejection: rejection.clone() };

    // Don't let an unreachable host hang for minutes.
    let attempt = tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, (profile.host.as_str(), profile.port), handler)).await;
    let mut handle = match attempt {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            let rejected = rejection.lock().unwrap().take();
            return Err(rejected.unwrap_or_else(|| network_error(&profile.host, profile.port, e)));
        }
        Err(_) => return Err(network_error(&profile.host, profile.port, "timed out")),
    };

    let auth_label = authenticate(&mut handle, profile, password, default_vault).await?;

    let sftp = match open_sftp(&handle).await {
        Ok(sftp) => Some(sftp),
        // An SSH profile is still useful for its terminal without SFTP.
        Err(_) if profile.protocol == Protocol::Ssh => None,
        Err(e) => return Err(e),
    };
    let remote_home = match &sftp {
        Some(sftp) => Some(sftp.canonicalize(".").await?),
        None => None,
    };
    let home = remote_home.clone().unwrap_or_else(|| "~".into());
    let session = Session {
        ssh: Some(handle),
        fs: sftp.map(RemoteFs::Sftp),
        server_id: profile.id.clone(),
        server_name: profile.name.clone(),
        remote_home,
    };
    Ok((session, home, auth_label))
}

async fn open_sftp(handle: &Handle<Client>) -> AppResult<SftpSession> {
    let channel = handle.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    // 64 writes of 32 KiB in flight, like OpenSSH's sftp: the default 16 caps
    // uploads at 512 KiB per round trip. 32 KiB is the size every server accepts.
    // The request timeout (10 s by default) starts when a request is queued, not sent: with many
    // files of 64 writes each queued on a slow uplink, the default fails healthy uploads and listings.
    let config = russh_sftp::client::Config { max_concurrent_writes: 64, request_timeout_secs: 120, ..Default::default() };
    Ok(SftpSession::new_with_config(channel.into_stream(), config).await?)
}

pub fn join_remote(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub async fn list(session: &Session, path: &str) -> AppResult<Vec<Entry>> {
    session.fs()?.list(path).await
}

pub async fn mkdir(session: &Session, path: &str) -> AppResult<()> {
    session.fs()?.mkdir(path).await
}

pub async fn create_file(session: &Session, path: &str) -> AppResult<()> {
    session.fs()?.create_new(path).await
}

pub async fn rename(session: &Session, from: &str, to: &str) -> AppResult<()> {
    let fs = session.fs()?;
    if fs.exists(to).await? {
        return Err(AppError::other(tr!("{to} already exists", "{to} bestaat al", to = to)));
    }
    fs.rename(from, to).await
}

/// Delete a file, symlink or directory tree. Symlinks are removed, never followed.
pub async fn delete(session: &Session, path: &str) -> AppResult<()> {
    session.fs()?.delete_tree(path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(auth: Auth) -> ServerProfile {
        ServerProfile {
            id: "c".into(),
            name: "c".into(),
            protocol: Protocol::Ssh,
            host: "h".into(),
            port: 22,
            user: "u".into(),
            group: String::new(),
            auth,
            remote_path: None,
            local_path: None,
            workspace: String::new(),
            tunnels: Vec::new(),
            updated_at: 0,
        }
    }

    fn op(fp: Option<&str>, account: Option<&str>, item: Option<&str>) -> Auth {
        Auth::OnePassword { key_fingerprint: fp.map(Into::into), account: account.map(Into::into), key_item: item.map(Into::into) }
    }

    fn ws(vault: Option<&str>, key: Option<(&str, &str)>) -> Workspace {
        Workspace {
            op_account: Some("ACC".into()),
            op_vault: vault.map(Into::into),
            op_key_fingerprint: key.map(|k| k.0.into()),
            op_key_item: key.map(|k| k.1.into()),
            ..Workspace::fallback()
        }
    }

    fn parts(p: &ServerProfile) -> (Option<String>, Option<String>, Option<String>) {
        match &p.auth {
            Auth::OnePassword { key_fingerprint, account, key_item } => (key_fingerprint.clone(), account.clone(), key_item.clone()),
            _ => panic!("not 1Password"),
        }
    }

    #[test]
    fn connection_key_wins_over_workspace_defaults() {
        let w = ws(Some("V"), Some(("SHA256:ws", "op://V/ws")));
        let (p, vault) = apply_workspace_defaults(&conn(op(Some("SHA256:own"), None, Some("op://V/own"))), Some(&w));
        assert_eq!(parts(&p), (Some("SHA256:own".into()), Some("ACC".into()), Some("op://V/own".into())));
        assert_eq!(vault, None);
    }

    #[test]
    fn workspace_key_is_the_only_key() {
        let w = ws(Some("V"), Some(("SHA256:ws", "op://V/ws")));
        let (p, vault) = apply_workspace_defaults(&conn(op(None, None, None)), Some(&w));
        assert_eq!(parts(&p), (Some("SHA256:ws".into()), Some("ACC".into()), Some("op://V/ws".into())));
        assert_eq!(vault, None, "a default key makes the vault filter redundant");
    }

    #[test]
    fn workspace_vault_alone_limits_to_the_vault() {
        let (p, vault) = apply_workspace_defaults(&conn(op(None, None, None)), Some(&ws(Some("V"), None)));
        assert_eq!(parts(&p), (None, Some("ACC".into()), None));
        assert_eq!(vault.as_deref(), Some("V"));
    }

    #[test]
    fn nothing_configured_changes_nothing() {
        let (p, vault) = apply_workspace_defaults(&conn(op(None, None, None)), Some(&ws(None, None)));
        assert_eq!(parts(&p), (None, Some("ACC".into()), None));
        assert_eq!(vault, None);
        let (p, vault) = apply_workspace_defaults(&conn(op(None, None, None)), None);
        assert_eq!(parts(&p), (None, None, None));
        assert_eq!(vault, None);
    }

    #[test]
    fn defaults_stay_within_the_workspace_account() {
        let w = ws(Some("V"), Some(("SHA256:ws", "op://V/ws")));
        let (p, vault) = apply_workspace_defaults(&conn(op(None, Some("OTHER"), None)), Some(&w));
        assert_eq!(parts(&p), (None, Some("OTHER".into()), None));
        assert_eq!(vault, None);
    }

    #[test]
    fn workspace_without_account_applies_to_any_account() {
        let w = Workspace { op_account: None, ..ws(Some("V"), None) };
        let (p, vault) = apply_workspace_defaults(&conn(op(None, Some("ANY"), None)), Some(&w));
        assert_eq!(parts(&p), (None, Some("ANY".into()), None));
        assert_eq!(vault.as_deref(), Some("V"));
    }
}
