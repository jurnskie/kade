//! Persistence for connections and settings.
//!
//! Everything shareable lives in one `kade.json`. By default that file sits in
//! the local config dir; the user can point Kade at a folder that a sync client
//! (Synology Drive, Syncthing, Dropbox, iCloud…) keeps in step across machines.
//! Which folder that is, is the only per-machine state (`config.json`).
//!
//! A JSON file replaced atomically survives sync clients far better than a
//! database. Concurrent edits on two machines are reconciled per connection by
//! `updated_at`, with tombstones so deletions propagate too.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::profiles::ServerProfile;

pub const DATA_FILE: &str = "kade.json";
const CONFIG_FILE: &str = "config.json";
const LEGACY_FILE: &str = "servers.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Show dotfiles in file panes by default.
    #[serde(default)]
    pub show_hidden: bool,
    /// How long backups of deleted/overwritten files are kept.
    #[serde(default = "default_retention")]
    pub backup_retention_days: u32,
    #[serde(default)]
    pub updated_at: i64,
}

fn default_retention() -> u32 {
    7
}

impl Default for Settings {
    fn default() -> Self {
        Settings { show_hidden: false, backup_retention_days: default_retention(), updated_at: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Data {
    pub version: u32,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub servers: Vec<ServerProfile>,
    /// Deleted server/workspace id → deletion time (unix ms), so deletes win over stale copies.
    #[serde(default)]
    pub deleted: HashMap<String, i64>,
    #[serde(default)]
    pub settings: Settings,
}

/// A top-level area such as "Thuis" or "Werk", above the groups.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    /// Palette key: pine, blue, amber, plum, coral or slate.
    #[serde(default = "default_color")]
    pub color: String,
    /// 1Password account used by this workspace's connections unless they name their own.
    #[serde(default)]
    pub op_account: Option<String>,
    #[serde(default)]
    pub updated_at: i64,
}

fn default_color() -> String {
    "pine".into()
}

/// Connections without a workspace belong here. A fixed id, so two machines
/// creating it independently don't end up with two "Thuis" workspaces.
pub const DEFAULT_WORKSPACE: &str = "default";

impl Workspace {
    pub fn fallback() -> Self {
        Workspace { id: DEFAULT_WORKSPACE.into(), name: "Thuis".into(), color: default_color(), op_account: None, updated_at: 0 }
    }
}

impl Data {
    /// Workspaces to show: the stored ones, or the implicit default.
    pub fn workspaces_or_default(&self) -> Vec<Workspace> {
        let mut out = self.workspaces.clone();
        let default_used = self.servers.iter().any(|s| s.workspace.is_empty() || s.workspace == DEFAULT_WORKSPACE);
        if out.is_empty() || (default_used && !out.iter().any(|w| w.id == DEFAULT_WORKSPACE)) {
            out.insert(0, Workspace::fallback());
        }
        out
    }

    pub fn workspace_of(&self, profile: &ServerProfile) -> Option<Workspace> {
        let id = if profile.workspace.is_empty() { DEFAULT_WORKSPACE } else { profile.workspace.as_str() };
        self.workspaces_or_default().into_iter().find(|w| w.id == id)
    }
}

impl Default for Data {
    fn default() -> Self {
        Data { version: 1, workspaces: Vec::new(), servers: Vec::new(), deleted: HashMap::new(), settings: Settings::default() }
    }
}

/// Per-machine config: only where the shared data lives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConfig {
    #[serde(default)]
    pub sync_dir: Option<PathBuf>,
    /// Command to open files with (`code`, `ghostty -e nvim`, …); empty picks a
    /// sensible default. Per machine: editors differ between computers.
    #[serde(default)]
    pub editor: String,
    /// Built-in MCP server for AI assistants (off by default).
    #[serde(default)]
    pub mcp_enabled: bool,
    #[serde(default = "default_mcp_port")]
    pub mcp_port: u16,
    /// Bearer token MCP clients must send; generated on first enable.
    #[serde(default)]
    pub mcp_token: String,
    /// "en", "nl", or empty to follow the system language.
    #[serde(default)]
    pub language: String,
}

fn default_mcp_port() -> u16 {
    7311
}

pub fn save_local(cfg: &LocalConfig) -> AppResult<()> {
    save_local_config(cfg)
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncStatus {
    pub sync_dir: Option<String>,
    pub data_file: String,
    /// Conflict copies from the sync client merged during the last load.
    pub merged_conflicts: Vec<String>,
}

/// Serialises read-modify-write cycles and remembers what we last wrote, so the
/// file watcher can tell our own writes from another machine's.
struct Inner {
    last_written: Option<String>,
    merged_conflicts: Vec<String>,
}

static STATE: Mutex<Inner> = Mutex::new(Inner { last_written: None, merged_conflicts: Vec::new() });

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn config_dir() -> AppResult<PathBuf> {
    let dir = dirs::config_dir().ok_or_else(|| AppError::other(tr!("No config directory found", "Geen config-map gevonden")))?.join("kade");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

impl Default for LocalConfig {
    fn default() -> Self {
        LocalConfig {
            sync_dir: None,
            editor: String::new(),
            mcp_enabled: false,
            mcp_port: default_mcp_port(),
            mcp_token: String::new(),
            language: String::new(),
        }
    }
}

pub fn local_config() -> AppResult<LocalConfig> {
    let path = config_dir()?.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(LocalConfig::default());
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn save_local_config(cfg: &LocalConfig) -> AppResult<()> {
    write_atomic(&config_dir()?.join(CONFIG_FILE), &serde_json::to_string_pretty(cfg)?)
}

pub fn set_editor(editor: &str) -> AppResult<()> {
    let mut cfg = local_config()?;
    cfg.editor = editor.trim().to_string();
    save_local_config(&cfg)
}

pub fn data_dir() -> AppResult<PathBuf> {
    match local_config()?.sync_dir {
        Some(dir) => Ok(dir),
        None => config_dir(),
    }
}

fn write_atomic(path: &Path, contents: &str) -> AppResult<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn read_data(path: &Path) -> AppResult<Data> {
    let raw = std::fs::read_to_string(path)?;
    serde_json::from_str(&raw).map_err(|e| AppError::other(format!("{} is ongeldig: {e}", path.display())))
}

/// Conflict copies the common sync clients leave next to a file, e.g.
/// `kade (conflicted copy …).json`, `kade.sync-conflict-….json`, `kade_HOST_Conflict.json`.
fn conflict_copies(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_lowercase();
            name.starts_with("kade") && name.ends_with(".json") && name != DATA_FILE && name.contains("conflict")
        })
        .collect()
}

/// Merge two copies: per server the newest `updated_at` wins; a tombstone wins
/// when it is newer than the server's last edit.
trait Versioned {
    fn key(&self) -> &str;
    fn version(&self) -> i64;
}

impl Versioned for ServerProfile {
    fn key(&self) -> &str {
        &self.id
    }
    fn version(&self) -> i64 {
        self.updated_at
    }
}

impl Versioned for Workspace {
    fn key(&self) -> &str {
        &self.id
    }
    fn version(&self) -> i64 {
        self.updated_at
    }
}

/// Per item the newest version wins; a newer tombstone removes it. Order of
/// first appearance is kept.
fn merge_items<T: Versioned>(a: Vec<T>, b: Vec<T>, deleted: &HashMap<String, i64>) -> Vec<T> {
    let mut items: HashMap<String, T> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for item in a.into_iter().chain(b) {
        let key = item.key().to_string();
        match items.get(&key) {
            Some(existing) if existing.version() >= item.version() => {}
            Some(_) => {
                items.insert(key, item);
            }
            None => {
                order.push(key.clone());
                items.insert(key, item);
            }
        }
    }
    order.into_iter().filter_map(|k| items.remove(&k)).filter(|i| deleted.get(i.key()).is_none_or(|&t| t < i.version())).collect()
}

/// Merge two copies: per server/workspace the newest `updated_at` wins; a
/// tombstone wins when it is newer than the item's last edit.
pub fn merge(a: Data, b: Data) -> Data {
    let mut deleted = a.deleted;
    for (id, t) in b.deleted {
        let e = deleted.entry(id).or_insert(t);
        *e = (*e).max(t);
    }
    let servers = merge_items(a.servers, b.servers, &deleted);
    let workspaces = merge_items(a.workspaces, b.workspaces, &deleted);
    let settings = if b.settings.updated_at > a.settings.updated_at { b.settings } else { a.settings };
    Data { version: 1, workspaces, servers, deleted, settings }
}

fn save_locked(inner: &mut Inner, dir: &Path, data: &Data) -> AppResult<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(data)?;
    write_atomic(&dir.join(DATA_FILE), &json)?;
    inner.last_written = Some(json);
    Ok(())
}

fn load_locked(inner: &mut Inner, dir: &Path) -> AppResult<Data> {
    let path = dir.join(DATA_FILE);
    let mut data = if path.exists() {
        read_data(&path)?
    } else {
        // First run after upgrading: pick up the old local servers.json.
        let legacy = config_dir()?.join(LEGACY_FILE);
        if legacy.exists() {
            let servers: Vec<ServerProfile> = serde_json::from_str(&std::fs::read_to_string(&legacy)?)?;
            let data = Data { servers, ..Data::default() };
            save_locked(inner, dir, &data)?;
            std::fs::rename(&legacy, legacy.with_extension("json.migrated"))?;
            data
        } else {
            Data::default()
        }
    };

    // Fold in conflict copies, then set them aside (renamed, never deleted).
    let mut merged = Vec::new();
    for copy in conflict_copies(dir) {
        if let Ok(other) = read_data(&copy) {
            data = merge(data, other);
            let aside = copy.with_extension(format!("merged-{}", now_ms()));
            std::fs::rename(&copy, aside)?;
            merged.push(copy.file_name().unwrap_or_default().to_string_lossy().into_owned());
        }
    }
    if !merged.is_empty() {
        save_locked(inner, dir, &data)?;
        inner.merged_conflicts = merged;
    }
    Ok(data)
}

pub fn load() -> AppResult<Data> {
    let mut inner = STATE.lock().unwrap();
    load_locked(&mut inner, &data_dir()?)
}

/// Load, apply `f`, save — atomically with respect to other callers.
pub fn update<T>(f: impl FnOnce(&mut Data) -> AppResult<T>) -> AppResult<T> {
    let mut inner = STATE.lock().unwrap();
    let dir = data_dir()?;
    let mut data = load_locked(&mut inner, &dir)?;
    let out = f(&mut data)?;
    save_locked(&mut inner, &dir, &data)?;
    Ok(out)
}

/// True when `contents` is not what this process last wrote.
pub fn is_foreign_change(contents: &str) -> bool {
    STATE.lock().unwrap().last_written.as_deref() != Some(contents)
}

pub fn status() -> AppResult<SyncStatus> {
    let cfg = local_config()?;
    let dir = data_dir()?;
    Ok(SyncStatus {
        sync_dir: cfg.sync_dir.map(|p| p.to_string_lossy().into_owned()),
        data_file: dir.join(DATA_FILE).to_string_lossy().into_owned(),
        merged_conflicts: STATE.lock().unwrap().merged_conflicts.clone(),
    })
}

/// Move the shared data to `target` (or back to the local config dir with
/// `None`). Existing data at the target is merged with ours, not overwritten.
pub fn set_sync_dir(target: Option<PathBuf>) -> AppResult<SyncStatus> {
    {
        let mut inner = STATE.lock().unwrap();
        let current_dir = data_dir()?;
        let current = load_locked(&mut inner, &current_dir)?;

        let target_dir = match &target {
            Some(dir) => {
                if !dir.is_dir() {
                    return Err(AppError::other(tr!("{} is not a folder", "{} is geen map", dir.display())));
                }
                dir.clone()
            }
            None => config_dir()?,
        };
        let existing = if target_dir.join(DATA_FILE).exists() { read_data(&target_dir.join(DATA_FILE))? } else { Data::default() };
        save_locked(&mut inner, &target_dir, &merge(existing, current))?;
        let mut cfg = local_config()?;
        cfg.sync_dir = target;
        save_local_config(&cfg)?;
        inner.merged_conflicts.clear();
    }
    status()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{Auth, Protocol};

    fn server(id: &str, name: &str, updated_at: i64) -> ServerProfile {
        ServerProfile {
            id: id.into(),
            name: name.into(),
            protocol: Protocol::Sftp,
            host: "h".into(),
            port: 22,
            user: "u".into(),
            group: String::new(),
            auth: Auth::Password,
            remote_path: None,
            local_path: None,
            workspace: String::new(),
            tunnels: Vec::new(),
            updated_at,
        }
    }

    fn data(servers: Vec<ServerProfile>, deleted: &[(&str, i64)]) -> Data {
        Data { servers, deleted: deleted.iter().map(|(k, v)| (k.to_string(), *v)).collect(), ..Data::default() }
    }

    #[test]
    fn newest_edit_wins() {
        let a = data(vec![server("1", "oud", 10)], &[]);
        let b = data(vec![server("1", "new", 20)], &[]);
        let m = merge(a, b);
        assert_eq!(m.servers.len(), 1);
        assert_eq!(m.servers[0].name, "new");
    }

    #[test]
    fn union_of_servers() {
        let m = merge(data(vec![server("1", "a", 1)], &[]), data(vec![server("2", "b", 1)], &[]));
        assert_eq!(m.servers.len(), 2);
    }

    #[test]
    fn newer_delete_beats_older_edit() {
        let a = data(vec![server("1", "a", 10)], &[]);
        let b = data(vec![], &[("1", 20)]);
        assert!(merge(a, b).servers.is_empty());
    }

    #[test]
    fn workspaces_merge_like_servers() {
        let ws =
            |name: &str, t: i64| Workspace { id: "w".into(), name: name.into(), color: "pine".into(), op_account: None, updated_at: t };
        let a = Data { workspaces: vec![ws("Werk", 10)], ..Data::default() };
        let b = Data { workspaces: vec![ws("Kantoor", 20)], ..Data::default() };
        assert_eq!(merge(a, b).workspaces[0].name, "Kantoor");
    }

    #[test]
    fn default_workspace_is_implicit() {
        let d = data(vec![server("1", "a", 1)], &[]);
        let ws = d.workspaces_or_default();
        assert_eq!((ws.len(), ws[0].id.as_str()), (1, DEFAULT_WORKSPACE));
        assert_eq!(d.workspace_of(&d.servers[0]).unwrap().name, "Thuis");
    }

    #[test]
    fn newer_edit_beats_older_delete() {
        let a = data(vec![server("1", "a", 30)], &[]);
        let b = data(vec![], &[("1", 20)]);
        assert_eq!(merge(a, b).servers.len(), 1);
    }
}
