//! Built-in MCP server, so AI assistants can manage Kade's groups and
//! connections while the app runs.
//!
//! Listens on 127.0.0.1 only and requires `Authorization: Bearer <token>`; off
//! by default. Changes go through the same store as the UI (merge-safe, synced
//! via kade.json) and the window is told to reload right away.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::{Request, State as AxumState};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, IntoContents, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::profiles::{self, Auth, Protocol, ServerProfile};
use crate::store;

// ---- Tool parameters --------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListParams {
    /// Only connections in this group (case-insensitive).
    #[serde(default)]
    pub group: Option<String>,
    /// Only this workspace (name or id, e.g. "Werk").
    #[serde(default)]
    pub workspace: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceFilter {
    /// Only this workspace (name or id); omit for all.
    #[serde(default)]
    pub workspace: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NewConnection {
    /// Display name, e.g. "example.com" or "homelab".
    pub name: String,
    /// sftp (files + terminal), ssh (terminal only), ftps or ftp.
    pub protocol: Protocol,
    /// Hostname or IP address.
    pub host: String,
    /// Defaults to 22 for sftp/ssh, 21 for ftp/ftps (990 = implicit FTPS).
    #[serde(default)]
    pub port: Option<u16>,
    pub user: String,
    /// Group shown in the sidebar, e.g. "Homelab", "Web servers", "Clients".
    #[serde(default)]
    pub group: Option<String>,
    /// Workspace name or id from list_workspaces (e.g. "Home", "Work"); omit for the default one.
    #[serde(default)]
    pub workspace: Option<String>,
    /// How to log in. Prefer one_password (SSH key in 1Password) for SSH and
    /// one_password_secret (password item in 1Password) for FTP. Never put a
    /// plain password here: method "password" asks the user at connect time.
    pub auth: Auth,
    /// Folder to open on the server, e.g. /var/www/example.com (a "site").
    #[serde(default)]
    pub remote_path: Option<String>,
    /// Local folder to show next to it, e.g. ~/Sites/example.
    #[serde(default)]
    pub local_path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateConnection {
    /// Id from list_connections.
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub protocol: Option<Protocol>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    /// Move to another workspace (name or id).
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub auth: Option<Auth>,
    /// Empty string clears it.
    #[serde(default)]
    pub remote_path: Option<String>,
    /// Empty string clears it.
    #[serde(default)]
    pub local_path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IdParam {
    pub id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveParams {
    pub ids: Vec<String>,
    /// Target group; empty string removes them from any group.
    pub group: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenameGroupParams {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KeysParams {
    /// "one_password" (1Password SSH agent) or "agent" (SSH_AUTH_SOCK).
    pub source: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LoginsParams {
    /// Filter on title, username or URL, e.g. "example".
    #[serde(default)]
    pub query: Option<String>,
    /// 1Password account id from list_1password_vaults; omit for the default account.
    #[serde(default)]
    pub account: Option<String>,
    /// Vault id or name to search in; omit for all vaults.
    #[serde(default)]
    pub vault: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VaultParams {
    /// 1Password account id; omit for the default account.
    #[serde(default)]
    pub account: Option<String>,
    /// Vault id or name; omit for all vaults.
    #[serde(default)]
    pub vault: Option<String>,
}

// ---- Output shapes ----------------------------------------------------------

#[derive(Serialize)]
struct ConnectionOut<'a> {
    id: &'a str,
    name: &'a str,
    workspace: &'a str,
    group: &'a str,
    protocol: Protocol,
    host: &'a str,
    port: u16,
    user: &'a str,
    auth: &'a Auth,
    remote_path: Option<&'a str>,
    local_path: Option<&'a str>,
}

fn out(p: &ServerProfile) -> ConnectionOut<'_> {
    ConnectionOut {
        id: &p.id,
        name: &p.name,
        workspace: if p.workspace.is_empty() { store::DEFAULT_WORKSPACE } else { &p.workspace },
        group: &p.group,
        protocol: p.protocol,
        host: &p.host,
        port: p.port,
        user: &p.user,
        auth: &p.auth,
        remote_path: p.remote_path.as_deref(),
        local_path: p.local_path.as_deref(),
    }
}

/// A tool-level failure: reported to the model as a result, not a protocol
/// error. Any error converts, so tools can use `?`.
struct Failed(String);

impl<E: std::fmt::Display> From<E> for Failed {
    fn from(e: E) -> Self {
        Failed(e.to_string())
    }
}

impl IntoContents for Failed {
    fn into_contents(self) -> Vec<ContentBlock> {
        vec![ContentBlock::text(self.0)]
    }
}

type ToolResult = Result<CallToolResult, Failed>;

fn json(value: impl Serialize) -> ToolResult {
    Ok(CallToolResult::success(vec![ContentBlock::json(value)?]))
}

fn text(msg: impl Into<String>) -> ToolResult {
    Ok(CallToolResult::success(vec![ContentBlock::text(msg.into())]))
}

fn failed(e: impl std::fmt::Display) -> ToolResult {
    Err(e.into())
}

fn default_port(protocol: Protocol) -> u16 {
    match protocol {
        Protocol::Sftp | Protocol::Ssh => 22,
        Protocol::Ftp | Protocol::Ftps => 21,
    }
}

/// Workspace id for a name or id given by the assistant.
fn resolve_workspace(wanted: Option<&str>) -> Result<String, String> {
    let all = store::load().map_err(|e| e.to_string())?.workspaces_or_default();
    let Some(w) = wanted.map(str::trim).filter(|w| !w.is_empty()) else {
        return Ok(all.first().map(|w| w.id.clone()).unwrap_or_else(|| store::DEFAULT_WORKSPACE.into()));
    };
    all.iter().find(|ws| ws.id == w || ws.name.eq_ignore_ascii_case(w)).map(|ws| ws.id.clone()).ok_or_else(|| {
        let names: Vec<_> = all.iter().map(|w| w.name.as_str()).collect();
        format!("No workspace '{w}'. Existing: {}", names.join(", "))
    })
}

fn in_workspace(p: &ServerProfile, id: &str) -> bool {
    p.workspace == id || (p.workspace.is_empty() && id == store::DEFAULT_WORKSPACE)
}

fn blank_to_none(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

// ---- Server -----------------------------------------------------------------

#[derive(Clone)]
pub struct KadeMcp {
    app: AppHandle,
    tool_router: ToolRouter<Self>,
}

impl KadeMcp {
    /// Tell the window to reload connections (our own writes don't trigger the
    /// file watcher, which only reports other machines' changes).
    fn changed(&self) {
        let _ = self.app.emit("store-changed", ());
    }

    fn find(&self, id: &str) -> Result<ServerProfile, AppError> {
        profiles::get(id)
    }
}

#[tool_router]
impl KadeMcp {
    pub fn new(app: AppHandle) -> Self {
        Self { app, tool_router: Self::tool_router() }
    }

    #[tool(description = "List the workspaces (e.g. Home, Work), each with its default 1Password account and number of connections.")]
    async fn list_workspaces(&self) -> ToolResult {
        let data = store::load()?;
        json(
            data.workspaces_or_default()
                .iter()
                .map(|w| {
                    let n = data.servers.iter().filter(|s| in_workspace(s, &w.id)).count();
                    serde_json::json!({ "id": w.id, "name": w.name, "color": w.color, "op_account": w.op_account, "connections": n })
                })
                .collect::<Vec<_>>(),
        )
    }

    #[tool(description = "List groups with how many connections each has, optionally within one workspace.")]
    async fn list_groups(&self, Parameters(p): Parameters<WorkspaceFilter>) -> ToolResult {
        let ws = p.workspace.as_deref().map(|w| resolve_workspace(Some(w))).transpose()?;
        let servers: Vec<ServerProfile> =
            profiles::list()?.into_iter().filter(|s| ws.as_deref().is_none_or(|w| in_workspace(s, w))).collect();
        let mut groups: Vec<(String, usize)> = Vec::new();
        for s in &servers {
            match groups.iter_mut().find(|(g, _)| *g == s.group) {
                Some((_, n)) => *n += 1,
                None => groups.push((s.group.clone(), 1)),
            }
        }
        json(
            groups
                .into_iter()
                .map(|(name, connections)| serde_json::json!({ "name": name, "connections": connections }))
                .collect::<Vec<_>>(),
        )
    }

    #[tool(
        description = "List connections (servers and sites), optionally only one group. A 'site' is a connection whose remote_path points at the site's folder."
    )]
    async fn list_connections(&self, Parameters(p): Parameters<ListParams>) -> ToolResult {
        let servers = profiles::list()?;
        let wanted = p.group.map(|g| g.to_lowercase());
        let ws = p.workspace.as_deref().map(|w| resolve_workspace(Some(w))).transpose()?;
        json(
            servers
                .iter()
                .filter(|s| wanted.as_ref().is_none_or(|g| s.group.to_lowercase() == *g))
                .filter(|s| ws.as_deref().is_none_or(|w| in_workspace(s, w)))
                .map(out)
                .collect::<Vec<_>>(),
        )
    }

    #[tool(
        description = "Add a connection to Kade. Several sites on one server are separate connections with the same host/user/auth and their own remote_path, usually in a group named after the server. Returns the new connection; if an identical one exists, returns that instead."
    )]
    async fn add_connection(&self, Parameters(p): Parameters<NewConnection>) -> ToolResult {
        let port = p.port.unwrap_or_else(|| default_port(p.protocol));
        let workspace = resolve_workspace(p.workspace.as_deref())?;
        let remote_path = blank_to_none(p.remote_path);
        if let Ok(existing) = profiles::list() {
            if let Some(dup) = existing
                .iter()
                .find(|s| s.host.eq_ignore_ascii_case(&p.host) && s.port == port && s.user == p.user && s.remote_path == remote_path)
            {
                return json(serde_json::json!({ "already_exists": true, "connection": out(dup) }));
            }
        }
        let profile = ServerProfile {
            id: String::new(),
            name: p.name.trim().to_string(),
            protocol: p.protocol,
            host: p.host.trim().to_string(),
            port,
            user: p.user.trim().to_string(),
            group: p.group.unwrap_or_default().trim().to_string(),
            auth: p.auth,
            remote_path,
            local_path: blank_to_none(p.local_path),
            workspace,
            tunnels: Vec::new(),
            updated_at: 0,
        };
        let saved = profiles::upsert(profile)?;
        self.changed();
        json(serde_json::json!({ "added": out(&saved) }))
    }

    #[tool(description = "Change fields of an existing connection; omitted fields stay as they are.")]
    async fn update_connection(&self, Parameters(p): Parameters<UpdateConnection>) -> ToolResult {
        // Resolved up front: the store is locked while the change is applied.
        let workspace = p.workspace.as_deref().map(|v| resolve_workspace(Some(v))).transpose()?;
        let result = profiles::modify(&p.id, |c| {
            if let Some(v) = p.name {
                c.name = v.trim().into();
            }
            if let Some(v) = p.protocol {
                c.protocol = v;
            }
            if let Some(v) = p.host {
                c.host = v.trim().into();
            }
            if let Some(v) = p.port {
                c.port = v;
            }
            if let Some(v) = p.user {
                c.user = v.trim().into();
            }
            if let Some(v) = p.group {
                c.group = v.trim().into();
            }
            if let Some(id) = workspace {
                c.workspace = id;
            }
            if let Some(v) = p.auth {
                c.auth = v;
            }
            if p.remote_path.is_some() {
                c.remote_path = blank_to_none(p.remote_path);
            }
            if p.local_path.is_some() {
                c.local_path = blank_to_none(p.local_path);
            }
        });
        let saved = result?;
        self.changed();
        json(serde_json::json!({ "updated": out(&saved) }))
    }

    #[tool(description = "Delete a connection from Kade (only the saved connection; nothing on the server is touched).")]
    async fn delete_connection(&self, Parameters(p): Parameters<IdParam>) -> ToolResult {
        let c = self.find(&p.id)?;
        profiles::delete(&p.id)?;
        self.changed();
        text(format!("Connection '{}' removed from Kade.", c.name))
    }

    #[tool(description = "Move connections into a group (created implicitly).")]
    async fn move_to_group(&self, Parameters(p): Parameters<MoveParams>) -> ToolResult {
        let group = p.group.trim().to_string();
        let result = store::update(|data| {
            let now = store::now_ms();
            let mut moved = 0;
            for s in data.servers.iter_mut().filter(|s| p.ids.contains(&s.id)) {
                s.group = group.clone();
                s.updated_at = now;
                moved += 1;
            }
            Ok(moved)
        });
        let n = result?;
        self.changed();
        text(format!("Moved {n} connection(s) to group '{group}'."))
    }

    #[tool(description = "Rename a group (or merge it into another by renaming to an existing name).")]
    async fn rename_group(&self, Parameters(p): Parameters<RenameGroupParams>) -> ToolResult {
        let (from, to) = (p.from.trim().to_string(), p.to.trim().to_string());
        let result = store::update(|data| {
            let now = store::now_ms();
            let mut n = 0;
            for s in data.servers.iter_mut().filter(|s| s.group.eq_ignore_ascii_case(&from)) {
                s.group = to.clone();
                s.updated_at = now;
                n += 1;
            }
            Ok(n)
        });
        let n = result?;
        if n == 0 {
            return failed(format!("No group '{from}' found."));
        }
        self.changed();
        text(format!("Renamed group '{from}' to '{to}' ({n} connections)."))
    }

    #[tool(
        description = "Try to log in with a saved connection and report the result, without opening it in the app. Connections with method 'password' cannot be tested this way. An unknown host key is reported, never trusted automatically: the user must open the connection in Kade and check the fingerprint."
    )]
    async fn test_connection(&self, Parameters(p): Parameters<IdParam>) -> ToolResult {
        let profile = self.find(&p.id)?;
        match crate::ssh::connect(&profile, None, None).await {
            Ok((session, connected)) => {
                if let Some(fs) = &session.fs {
                    fs.close().await;
                }
                if let Some(ssh) = &session.ssh {
                    let _ = ssh.disconnect(russh::Disconnect::ByApplication, "", "nl").await;
                }
                json(serde_json::json!({
                    "ok": true,
                    "login": connected.auth_label,
                    "home": connected.home,
                    "files": connected.has_files,
                    "terminal": connected.has_terminal,
                }))
            }
            Err(e) => json(serde_json::json!({ "ok": false, "error": e })),
        }
    }

    #[tool(description = "Open a connection in the Kade window (connects and shows it in a tab).")]
    async fn open_connection(&self, Parameters(p): Parameters<IdParam>) -> ToolResult {
        let c = self.find(&p.id)?;
        let _ = self.app.emit("mcp-open", &c.id);
        text(format!("Opening '{}' in Kade.", c.name))
    }

    #[tool(
        description = "List SSH keys available for auth: source 'one_password' (1Password SSH agent) or 'agent'. Use a key's fingerprint as auth.key_fingerprint to pin it."
    )]
    async fn list_ssh_keys(&self, Parameters(p): Parameters<KeysParams>) -> ToolResult {
        let auth = match p.source.as_str() {
            "one_password" => Auth::OnePassword { key_fingerprint: None, account: None, key_item: None },
            "agent" => Auth::Agent { key_fingerprint: None },
            other => return failed(format!("Unknown source '{other}'; use one_password or agent.")),
        };
        json(crate::ssh::agent_keys(&auth).await?)
    }

    #[tool(
        description = "List the 1Password accounts on this computer and the vaults in each. Machines can be signed in to several accounts (work and personal): always set auth.account so Kade uses the right one."
    )]
    async fn list_1password_vaults(&self) -> ToolResult {
        let accounts = crate::onepassword::accounts().await?;
        let mut out = Vec::new();
        for a in accounts {
            let vaults = crate::onepassword::vaults(Some(&a.id)).await.unwrap_or_default();
            out.push(serde_json::json!({ "account": a, "vaults": vaults }));
        }
        json(out)
    }

    #[tool(
        description = "List SSH Key items in a 1Password account/vault with their fingerprints. For auth method one_password set key_fingerprint, key_item (the item reference) and account, so the key works even on machines whose SSH agent doesn't offer that vault."
    )]
    async fn find_1password_ssh_keys(&self, Parameters(p): Parameters<VaultParams>) -> ToolResult {
        json(crate::onepassword::ssh_keys(p.account.as_deref(), p.vault.as_deref()).await?)
    }

    #[tool(
        description = "Search 1Password logins (needs the 1Password CLI). Use a result's reference as auth.reference with method 'one_password_secret', together with auth.account. Never returns passwords."
    )]
    async fn find_1password_logins(&self, Parameters(p): Parameters<LoginsParams>) -> ToolResult {
        let items = crate::onepassword::items(p.account.as_deref(), p.vault.as_deref()).await?;
        let q = p.query.unwrap_or_default().to_lowercase();
        let hits: Vec<_> = items
            .into_iter()
            .filter(|i| {
                q.is_empty()
                    || format!("{} {} {}", i.title, i.username.as_deref().unwrap_or(""), i.url.as_deref().unwrap_or(""))
                        .to_lowercase()
                        .contains(&q)
            })
            .take(25)
            .collect();
        json(hits)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for KadeMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("kade", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Kade is the user's SFTP/SSH/FTP manager. Connections live in groups inside workspaces \
                 (e.g. Home, Work); a 'site' is a connection whose remote_path is the site's folder. \
                 A workspace's op_account is used for 1Password unless the connection names its own. Never store plain passwords: use \
                 one_password (SSH key) or one_password_secret (1Password item reference), or \
                 'password' to have Kade ask at connect time. Check list_connections before adding \
                 to avoid duplicates.",
            )
    }
}

// ---- Lifecycle --------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct McpStatus {
    pub enabled: bool,
    pub running: bool,
    pub url: String,
    pub token: String,
    pub error: Option<String>,
}

struct Running {
    port: u16,
    cancel: CancellationToken,
    task: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Default)]
pub struct McpServer {
    running: Mutex<Option<Running>>,
    /// Read on every request, so a new token applies without restarting.
    token: Arc<RwLock<String>>,
    error: Mutex<Option<String>>,
}

async fn require_token(AxumState(token): AxumState<Arc<RwLock<String>>>, req: Request, next: Next) -> Response {
    let given = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    if given != Some(token.read().unwrap().as_str()) {
        return Response::builder().status(StatusCode::UNAUTHORIZED).body("unauthorized".into()).unwrap();
    }
    next.run(req).await
}

impl McpServer {
    pub fn status(&self) -> McpStatus {
        let cfg = store::local_config().unwrap_or_default();
        McpStatus {
            enabled: cfg.mcp_enabled,
            running: self.running.lock().unwrap().is_some(),
            url: format!("http://127.0.0.1:{}/mcp", cfg.mcp_port),
            token: cfg.mcp_token,
            error: self.error.lock().unwrap().clone(),
        }
    }

    /// (Re)start according to the local config.
    pub async fn apply(&self, app: &AppHandle) {
        let cfg = store::local_config().unwrap_or_default();
        *self.token.write().unwrap() = cfg.mcp_token.clone();
        let wanted = cfg.mcp_enabled && !cfg.mcp_token.is_empty();
        if wanted && self.running.lock().unwrap().as_ref().is_some_and(|r| r.port == cfg.mcp_port) {
            return; // already serving; a new token is picked up per request
        }
        let old = self.running.lock().unwrap().take();
        if let Some(old) = old {
            old.cancel.cancel();
            old.task.abort();
            // Wait until the listener is dropped, so its port is free to bind again.
            let _ = old.task.await;
        }
        *self.error.lock().unwrap() = None;
        if !wanted {
            return;
        }
        let cancel = CancellationToken::new();
        let addr = SocketAddr::from(([127, 0, 0, 1], cfg.mcp_port));

        let mut config = StreamableHttpServerConfig::default();
        config.cancellation_token = cancel.child_token();
        let service = StreamableHttpService::new(
            {
                let app = app.clone();
                move || Ok(KadeMcp::new(app.clone()))
            },
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let router =
            axum::Router::new().nest_service("/mcp", service).layer(middleware::from_fn_with_state(self.token.clone(), require_token));

        // Bind here, not in the task, so a busy port is reported in the settings.
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                *self.error.lock().unwrap() =
                    Some(tr!("Port {} is not available: {}", "Poort {} is niet beschikbaar: {}", cfg.mcp_port, e));
                return;
            }
        };
        let task = tauri::async_runtime::spawn({
            let cancel = cancel.clone();
            async move {
                let _ = axum::serve(listener, router).with_graceful_shutdown(async move { cancel.cancelled().await }).await;
            }
        });
        *self.running.lock().unwrap() = Some(Running { port: cfg.mcp_port, cancel, task });
    }
}

/// Settings toggle: enabling creates a token the first time.
pub fn set_enabled(enabled: bool) -> Result<(), AppError> {
    store::update_local(|cfg| {
        cfg.mcp_enabled = enabled;
        if enabled && cfg.mcp_token.is_empty() {
            cfg.mcp_token = new_token();
        }
    })
}

pub fn regenerate_token() -> Result<(), AppError> {
    store::update_local(|cfg| cfg.mcp_token = new_token())
}

fn new_token() -> String {
    format!("kade_{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}
