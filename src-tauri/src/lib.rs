#[macro_use]
mod i18n;
mod backup;
#[cfg(test)]
mod e2e;
mod edit;
mod editors;
mod error;
mod fs;
mod ftp;
mod mcp;
mod onepassword;
mod profiles;
mod remote;
mod ssh;
mod store;
mod terminal;
mod transfer;
mod update;
mod watch;
mod workspace;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use error::{AppError, AppResult};
use profiles::{Auth, ServerProfile};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
struct Sessions(Mutex<HashMap<String, Arc<ssh::Session>>>);

#[derive(Default)]
struct Terminals(Mutex<HashMap<String, Arc<terminal::Terminal>>>);

impl Terminals {
    fn get(&self, id: &str) -> AppResult<Arc<terminal::Terminal>> {
        self.0.lock().unwrap().get(id).cloned().ok_or(AppError::SessionNotFound)
    }
}

impl Sessions {
    fn get(&self, id: &str) -> AppResult<Arc<ssh::Session>> {
        self.0.lock().unwrap().get(id).cloned().ok_or(AppError::SessionNotFound)
    }
}

#[tauri::command]
fn list_servers() -> AppResult<Vec<ServerProfile>> {
    profiles::list()
}

#[tauri::command]
fn save_server(profile: ServerProfile) -> AppResult<ServerProfile> {
    profiles::upsert(profile)
}

#[tauri::command]
fn delete_server(id: String) -> AppResult<()> {
    profiles::delete(&id)
}

#[tauri::command]
fn list_workspaces() -> AppResult<Vec<store::Workspace>> {
    Ok(store::load()?.workspaces_or_default())
}

#[tauri::command]
fn save_workspace(mut workspace: store::Workspace) -> AppResult<store::Workspace> {
    workspace::save(&mut workspace)?;
    Ok(workspace)
}

/// Delete a workspace; its connections move to `move_to` (another workspace id).
#[tauri::command]
fn delete_workspace(id: String, move_to: String) -> AppResult<()> {
    workspace::delete(&id, &move_to)
}

#[tauri::command]
fn get_settings() -> AppResult<store::Settings> {
    Ok(store::load()?.settings)
}

#[tauri::command]
fn save_settings(mut settings: store::Settings) -> AppResult<store::Settings> {
    settings.updated_at = store::now_ms();
    store::update(|data| {
        data.settings = settings.clone();
        Ok(settings)
    })
}

#[tauri::command]
fn sync_status() -> AppResult<store::SyncStatus> {
    store::status()
}

#[tauri::command]
fn set_sync_dir(app: AppHandle, watcher: State<'_, watch::StoreWatcher>, dir: Option<String>) -> AppResult<store::SyncStatus> {
    let status = store::set_sync_dir(dir.map(std::path::PathBuf::from))?;
    watcher.restart(&app)?;
    Ok(status)
}

#[tauri::command]
async fn agent_keys(auth: Auth) -> AppResult<Vec<ssh::AgentKey>> {
    ssh::agent_keys(&auth).await
}

#[tauri::command]
async fn connect(
    sessions: State<'_, Sessions>,
    server_id: String,
    password: Option<String>,
    accept_fingerprint: Option<String>,
) -> AppResult<ssh::Connected> {
    let profile = profiles::get(&server_id)?;
    let (session, connected) = ssh::connect(&profile, password, accept_fingerprint).await?;
    sessions.0.lock().unwrap().insert(connected.session_id.clone(), Arc::new(session));
    Ok(connected)
}

#[tauri::command]
async fn disconnect(
    sessions: State<'_, Sessions>,
    terminals: State<'_, Terminals>,
    edits: State<'_, edit::Edits>,
    session_id: String,
) -> AppResult<()> {
    terminals.0.lock().unwrap().retain(|_, t| t.session_id != session_id);
    edits.stop_session(&session_id);
    let session = sessions.0.lock().unwrap().remove(&session_id);
    if let Some(session) = session {
        if let Some(fs) = &session.fs {
            fs.close().await;
        }
        if let Some(ssh) = &session.ssh {
            let _ = ssh.disconnect(russh::Disconnect::ByApplication, "", "nl").await;
        }
    }
    Ok(())
}

#[tauri::command]
async fn remote_list(sessions: State<'_, Sessions>, session_id: String, path: String) -> AppResult<Vec<fs::Entry>> {
    let session = sessions.get(&session_id)?;
    ssh::list(&session, &path).await
}

#[tauri::command]
async fn terminal_open(
    sessions: State<'_, Sessions>,
    terminals: State<'_, Terminals>,
    session_id: String,
    cols: u32,
    rows: u32,
    events: Channel<terminal::TermEvent>,
) -> AppResult<String> {
    let session = sessions.get(&session_id)?;
    let term = terminal::open(&session, session_id, cols, rows, events).await?;
    let id = uuid::Uuid::new_v4().to_string();
    terminals.0.lock().unwrap().insert(id.clone(), Arc::new(term));
    Ok(id)
}

#[tauri::command]
async fn terminal_write(terminals: State<'_, Terminals>, term_id: String, data: String) -> AppResult<()> {
    let term = terminals.get(&term_id)?;
    term.writer.data_bytes(data.into_bytes()).await?;
    Ok(())
}

#[tauri::command]
async fn terminal_resize(terminals: State<'_, Terminals>, term_id: String, cols: u32, rows: u32) -> AppResult<()> {
    let term = terminals.get(&term_id)?;
    term.writer.window_change(cols, rows, 0, 0).await?;
    Ok(())
}

#[tauri::command]
async fn terminal_close(terminals: State<'_, Terminals>, term_id: String) -> AppResult<()> {
    let term = terminals.0.lock().unwrap().remove(&term_id);
    if let Some(term) = term {
        let _ = term.writer.close().await;
    }
    Ok(())
}

#[tauri::command]
async fn remote_mkdir(sessions: State<'_, Sessions>, session_id: String, path: String) -> AppResult<()> {
    ssh::mkdir(&*sessions.get(&session_id)?, &path).await
}

#[tauri::command]
async fn remote_create_file(sessions: State<'_, Sessions>, session_id: String, path: String) -> AppResult<()> {
    ssh::create_file(&*sessions.get(&session_id)?, &path).await
}

#[tauri::command]
async fn remote_rename(sessions: State<'_, Sessions>, session_id: String, from: String, to: String) -> AppResult<()> {
    ssh::rename(&*sessions.get(&session_id)?, &from, &to).await
}

/// Deleting moves items into a backup transaction instead of removing them.
#[tauri::command]
async fn remote_delete(sessions: State<'_, Sessions>, session_id: String, paths: Vec<String>) -> AppResult<Option<backup::Transaction>> {
    let session = sessions.get(&session_id)?;
    let mut undo =
        backup::Recorder::new(backup::Side::Remote, backup::Op::Delete, Some(&session), delete_summary(&paths, &session.server_name));
    for path in &paths {
        // Clearing out Kade's own backup folder really deletes.
        if backup::is_remote_backup_path(&session, path) {
            ssh::delete(&session, path).await?;
        } else {
            undo.stash(Some(&session), path).await?;
        }
    }
    undo.commit()
}

fn delete_summary(paths: &[String], place: &str) -> String {
    match paths {
        [one] => tr!("{} deleted on {}", "{} verwijderd op {}", one.rsplit('/').next().unwrap_or(one), place),
        many => tr!("{} items deleted on {}", "{} items verwijderd op {}", many.len(), place),
    }
}

#[tauri::command]
fn local_mkdir(path: String) -> AppResult<()> {
    fs::mkdir(&path)
}

#[tauri::command]
fn local_create_file(path: String) -> AppResult<()> {
    fs::create_file(&path)
}

#[tauri::command]
fn local_rename(from: String, to: String) -> AppResult<()> {
    fs::rename(&from, &to)
}

#[tauri::command]
async fn local_delete(paths: Vec<String>) -> AppResult<Option<backup::Transaction>> {
    let mut undo = backup::Recorder::new(
        backup::Side::Local,
        backup::Op::Delete,
        None,
        delete_summary(&paths, &tr!("this computer", "deze computer")),
    );
    for path in &paths {
        undo.stash(None, path).await?;
    }
    undo.commit()
}

fn editor_setting() -> String {
    store::local_config().map(|c| c.editor).unwrap_or_default()
}

#[derive(serde::Serialize)]
struct EditorChoice {
    /// The configured command; empty means automatic.
    current: String,
    /// What "automatic" resolves to here, e.g. "Omarchy-editor (nvim)".
    automatic: String,
    options: Vec<editors::EditorOption>,
}

/// `language` is what the window shows ("en"/"nl"), so backend messages match
/// it; `remember` is the user's choice ("en", "nl" or "auto"), kept per machine.
#[tauri::command]
fn set_language(language: String, remember: Option<String>) -> AppResult<()> {
    i18n::set(&language);
    if let Some(choice) = remember {
        let choice = if choice == "auto" { String::new() } else { choice };
        let mut cfg = store::local_config()?;
        if cfg.language != choice {
            cfg.language = choice;
            store::save_local(&cfg)?;
        }
    }
    Ok(())
}

#[tauri::command]
fn get_language() -> String {
    let lang = store::local_config().map(|c| c.language).unwrap_or_default();
    if lang.is_empty() {
        "auto".into()
    } else {
        lang
    }
}

#[tauri::command]
fn get_editor() -> AppResult<EditorChoice> {
    let automatic = match editors::omarchy_editor() {
        Some(name) => tr!("Omarchy editor ({name})", "Omarchy-editor ({name})", name = name),
        None => tr!("The system's default app", "Standaardprogramma van het systeem"),
    };
    Ok(EditorChoice { current: store::local_config()?.editor, automatic, options: editors::detect() })
}

#[tauri::command]
fn set_editor(editor: String) -> AppResult<()> {
    store::set_editor(&editor)
}

#[tauri::command]
async fn edit_open(
    app: AppHandle,
    sessions: State<'_, Sessions>,
    edits: State<'_, edit::Edits>,
    session_id: String,
    path: String,
) -> AppResult<edit::EditInfo> {
    let session = sessions.get(&session_id)?;
    let emit: edit::Emit = Arc::new(move |e: &edit::EditInfo| {
        let _ = app.emit("edit", e);
    });
    edits.open(emit, session, session_id, path, &editor_setting()).await
}

#[tauri::command]
async fn edit_force(edits: State<'_, edit::Edits>, id: String) -> AppResult<()> {
    edits.force_upload(&id).await
}

#[tauri::command]
async fn edit_reload(edits: State<'_, edit::Edits>, id: String) -> AppResult<()> {
    edits.reload(&id).await
}

#[tauri::command]
fn edit_stop(edits: State<'_, edit::Edits>, id: String) {
    edits.stop(&id);
}

#[tauri::command]
fn edits_list(edits: State<'_, edit::Edits>) -> Vec<edit::EditInfo> {
    edits.list()
}

/// Local files open straight in the editor; no syncing needed.
#[tauri::command]
fn open_local(path: String) -> AppResult<()> {
    edit::launch(&editor_setting(), std::path::Path::new(&path))
}

#[tauri::command]
async fn update_check(app: AppHandle) -> update::UpdateInfo {
    update::check(&app.package_info().version.to_string()).await
}

#[tauri::command]
async fn update_install(app: AppHandle, version: String) -> AppResult<()> {
    update::install(app, version).await
}

#[tauri::command]
fn mcp_status(mcp: State<'_, mcp::McpServer>) -> mcp::McpStatus {
    mcp.status()
}

#[tauri::command]
fn mcp_set_enabled(app: AppHandle, mcp: State<'_, mcp::McpServer>, enabled: bool) -> AppResult<mcp::McpStatus> {
    mcp::set_enabled(enabled)?;
    mcp.apply(&app);
    Ok(mcp.status())
}

#[tauri::command]
fn mcp_regenerate_token(app: AppHandle, mcp: State<'_, mcp::McpServer>) -> AppResult<mcp::McpStatus> {
    mcp::regenerate_token()?;
    mcp.apply(&app);
    Ok(mcp.status())
}

#[tauri::command]
async fn op_accounts() -> AppResult<Vec<onepassword::OpAccount>> {
    onepassword::accounts().await
}

#[tauri::command]
async fn op_vaults(account: Option<String>) -> AppResult<Vec<onepassword::OpVault>> {
    onepassword::vaults(account.as_deref()).await
}

#[tauri::command]
async fn op_items(account: Option<String>, vault: Option<String>) -> AppResult<Vec<onepassword::OpItem>> {
    onepassword::items(account.as_deref(), vault.as_deref()).await
}

#[tauri::command]
async fn op_ssh_keys(account: Option<String>, vault: Option<String>) -> AppResult<Vec<onepassword::OpSshKey>> {
    onepassword::ssh_keys(account.as_deref(), vault.as_deref()).await
}

#[tauri::command]
fn backups_list() -> AppResult<Vec<backup::Transaction>> {
    backup::list()
}

#[tauri::command]
async fn backup_restore(sessions: State<'_, Sessions>, id: String, session_id: Option<String>) -> AppResult<Option<backup::Transaction>> {
    let session = session_id.map(|s| sessions.get(&s)).transpose()?;
    backup::restore(&id, session.as_deref()).await
}

#[tauri::command]
async fn backup_delete(sessions: State<'_, Sessions>, id: String, session_id: Option<String>) -> AppResult<()> {
    let session = session_id.map(|s| sessions.get(&s)).transpose()?;
    backup::delete(&id, session.as_deref()).await
}

#[tauri::command]
async fn transfer_conflicts(
    sessions: State<'_, Sessions>,
    session_id: String,
    direction: transfer::Direction,
    sources: Vec<String>,
    dest_dir: String,
) -> AppResult<Vec<String>> {
    let session = sessions.get(&session_id)?;
    transfer::conflicts(&session, direction, &sources, &dest_dir).await
}

#[tauri::command]
fn transfer_start(
    app: AppHandle,
    sessions: State<'_, Sessions>,
    transfers: State<'_, Arc<transfer::Transfers>>,
    session_id: String,
    direction: transfer::Direction,
    sources: Vec<String>,
    dest_dir: String,
    conflict: transfer::Conflict,
) -> AppResult<Vec<String>> {
    let session = sessions.get(&session_id)?;
    let emit: transfer::Emit = Arc::new(move |p: &transfer::Progress| {
        let _ = app.emit("transfer", p);
    });
    Ok(transfer::start(emit, transfers.inner().clone(), session, session_id, direction, sources, dest_dir, conflict))
}

#[tauri::command]
fn transfer_pause(transfers: State<'_, Arc<transfer::Transfers>>, id: String, paused: bool) {
    transfers.pause(&id, paused);
}

#[tauri::command]
fn transfer_cancel(transfers: State<'_, Arc<transfer::Transfers>>, id: String) {
    transfers.cancel(&id);
}

#[tauri::command]
fn local_home() -> String {
    fs::home()
}

#[tauri::command]
fn local_list(path: String) -> AppResult<Vec<fs::Entry>> {
    fs::list(&path)
}

/// Apps started from Finder or the Dock get a minimal PATH, without Homebrew.
/// Add the usual places so `op` (1Password CLI) and editors like `code` are found.
#[cfg(target_os = "macos")]
fn extend_path() {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    let mut extra = vec![std::path::PathBuf::from("/opt/homebrew/bin"), std::path::PathBuf::from("/usr/local/bin")];
    if let Some(home) = dirs::home_dir() {
        extra.push(home.join(".local/bin"));
        extra.push(home.join(".local/share/mise/shims"));
    }
    for dir in extra {
        if dir.is_dir() && !paths.contains(&dir) {
            paths.push(dir);
        }
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        // Called once at startup, before any other thread exists.
        std::env::set_var("PATH", joined);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "macos")]
    extend_path();
    i18n::set(&store::local_config().map(|c| c.language).unwrap_or_default());
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Sessions::default())
        .manage(Terminals::default())
        .manage(watch::StoreWatcher::default())
        .manage(Arc::new(transfer::Transfers::default()))
        .manage(edit::Edits::default())
        .manage(mcp::McpServer::default())
        .setup(|app| {
            // A missing sync folder (e.g. an unmounted drive) must not stop the app.
            if let Err(e) = app.state::<watch::StoreWatcher>().restart(app.handle()) {
                eprintln!("kade: can't watch the data folder: {e}");
            }
            app.state::<mcp::McpServer>().apply(app.handle());
            // Expired local backups can go right away; server ones on next connect.
            tauri::async_runtime::spawn(async {
                let days = store::load().map(|d| d.settings.backup_retention_days).unwrap_or(7);
                if let Err(e) = backup::purge(days, None).await {
                    eprintln!("kade: oude backups opruimen mislukt: {e}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_servers,
            save_server,
            delete_server,
            list_workspaces,
            save_workspace,
            delete_workspace,
            get_settings,
            save_settings,
            sync_status,
            set_sync_dir,
            agent_keys,
            connect,
            disconnect,
            remote_list,
            terminal_open,
            terminal_write,
            terminal_resize,
            terminal_close,
            remote_mkdir,
            remote_create_file,
            remote_rename,
            remote_delete,
            local_mkdir,
            local_create_file,
            local_rename,
            local_delete,
            op_accounts,
            op_vaults,
            op_items,
            op_ssh_keys,
            update_check,
            update_install,
            mcp_status,
            mcp_set_enabled,
            mcp_regenerate_token,
            edit_open,
            edit_force,
            edit_reload,
            edit_stop,
            edits_list,
            set_language,
            get_language,
            get_editor,
            set_editor,
            open_local,
            backups_list,
            backup_restore,
            backup_delete,
            transfer_conflicts,
            transfer_start,
            transfer_pause,
            transfer_cancel,
            local_home,
            local_list,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
