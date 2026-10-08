use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::store;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Sftp,
    Ssh,
    Ftps,
    Ftp,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Auth {
    /// Keys served by the 1Password SSH agent. `key_fingerprint` pins one key
    /// so we don't burn through the server's MaxAuthTries offering every key.
    ///
    /// `account` (1Password account id) and `key_item` (`op://vault/item` of
    /// the SSH Key) let Kade fetch the key itself when this machine's SSH
    /// agent doesn't offer it, e.g. a laptop where only the work account's
    /// vaults are enabled for the agent.
    OnePassword {
        key_fingerprint: Option<String>,
        #[serde(default)]
        account: Option<String>,
        #[serde(default)]
        key_item: Option<String>,
    },
    /// Keys from the agent in $SSH_AUTH_SOCK.
    Agent { key_fingerprint: Option<String> },
    /// A private key file, e.g. ~/.ssh/id_ed25519.
    KeyFile { path: String },
    /// Password, asked at connect time and never written to disk.
    Password,
    /// Password read from 1Password at connect time (`op://vault/item/password`).
    OnePasswordSecret {
        reference: String,
        /// 1Password account id; without it `op` uses its default account.
        #[serde(default)]
        account: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerProfile {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub user: String,
    #[serde(default)]
    pub group: String,
    pub auth: Auth,
    #[serde(default)]
    pub remote_path: Option<String>,
    #[serde(default)]
    pub local_path: Option<String>,
    /// Workspace id; empty means the default workspace.
    #[serde(default)]
    pub workspace: String,
    /// SSH tunnels (local port forwards) saved with this connection.
    #[serde(default)]
    pub tunnels: Vec<crate::tunnel::Tunnel>,
    /// Last edit (unix ms); the newest copy wins when synced files diverge.
    #[serde(default)]
    pub updated_at: i64,
}

pub fn list() -> AppResult<Vec<ServerProfile>> {
    Ok(store::load()?.servers)
}

/// Shared by the GUI and MCP save paths. The importer writes to the store directly, since
/// imported entries may lack a user (it is asked for at connect time).
fn validate(profile: &mut ServerProfile) -> AppResult<()> {
    profile.name = profile.name.trim().to_string();
    profile.host = profile.host.trim().to_string();
    profile.user = profile.user.trim().to_string();
    if profile.name.is_empty() {
        return Err(AppError::other(tr!("Name is required", "Naam is verplicht")));
    }
    if profile.host.is_empty() {
        return Err(AppError::other(tr!("Host is required", "Host is verplicht")));
    }
    if profile.user.is_empty() {
        return Err(AppError::other(tr!("User is required", "Gebruiker is verplicht")));
    }
    if profile.port == 0 {
        return Err(AppError::other(tr!("Port must be between 1 and 65535", "Poort moet tussen 1 en 65535 liggen")));
    }
    Ok(())
}

pub fn upsert(mut profile: ServerProfile) -> AppResult<ServerProfile> {
    validate(&mut profile)?;
    if profile.id.is_empty() {
        profile.id = uuid::Uuid::new_v4().to_string();
    }
    let mut ports = std::collections::HashSet::new();
    for t in &mut profile.tunnels {
        crate::tunnel::validate(t)?;
        if !ports.insert(t.local_port) {
            return Err(AppError::other(tr!(
                "Two tunnels use local port {port}",
                "Twee tunnels gebruiken lokale poort {port}",
                port = t.local_port
            )));
        }
        if t.id.is_empty() {
            t.id = uuid::Uuid::new_v4().to_string();
        }
    }
    profile.updated_at = store::now_ms();
    store::update(|data| {
        data.deleted.remove(&profile.id);
        match data.servers.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => *existing = profile.clone(),
            None => data.servers.push(profile.clone()),
        }
        Ok(profile)
    })
}

/// Change one saved connection in a single store transaction, so an edit made
/// elsewhere between reading and saving it isn't overwritten.
pub fn modify(id: &str, f: impl FnOnce(&mut ServerProfile)) -> AppResult<ServerProfile> {
    store::update(|data| {
        let profile = data
            .servers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| AppError::other(tr!("Connection not found", "Server niet gevonden")))?;
        f(profile);
        profile.updated_at = store::now_ms();
        Ok(profile.clone())
    })
}

pub fn delete(id: &str) -> AppResult<()> {
    store::update(|data| {
        data.servers.retain(|p| p.id != id);
        data.deleted.insert(id.to_string(), store::now_ms());
        Ok(())
    })
}

pub fn get(id: &str) -> AppResult<ServerProfile> {
    list()?.into_iter().find(|p| p.id == id).ok_or_else(|| AppError::other(tr!("Connection not found", "Server niet gevonden")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> ServerProfile {
        ServerProfile {
            id: String::new(),
            name: " prod ".into(),
            protocol: Protocol::Ssh,
            host: " example.com ".into(),
            port: 22,
            user: " deploy ".into(),
            group: String::new(),
            auth: Auth::Password,
            remote_path: None,
            local_path: None,
            workspace: String::new(),
            tunnels: Vec::new(),
            updated_at: 0,
        }
    }

    #[test]
    fn validate_trims_and_accepts() {
        let mut p = profile();
        validate(&mut p).unwrap();
        assert_eq!((p.name.as_str(), p.host.as_str(), p.user.as_str()), ("prod", "example.com", "deploy"));
    }

    #[test]
    fn validate_rejects_blank_fields_and_port_zero() {
        for edit in [
            (|p: &mut ServerProfile| p.name = "  ".into()) as fn(&mut ServerProfile),
            |p| p.host = String::new(),
            |p| p.user = " ".into(),
            |p| p.port = 0,
        ] {
            let mut p = profile();
            edit(&mut p);
            assert!(validate(&mut p).is_err());
        }
    }
}
