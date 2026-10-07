use std::sync::Mutex;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult};
use crate::store::{self, DATA_FILE};

/// Watches the data folder and tells the frontend when another machine (via
/// the sync client) changed `kade.json` or dropped a conflict copy next to it.
#[derive(Default)]
pub struct StoreWatcher(Mutex<Option<RecommendedWatcher>>);

fn is_data_file(name: &str) -> bool {
    let name = name.to_lowercase();
    name.starts_with("kade") && name.ends_with(".json")
}

impl StoreWatcher {
    /// (Re)start watching the current data folder.
    pub fn restart(&self, app: &AppHandle) -> AppResult<()> {
        let dir = store::data_dir()?;
        let file = dir.join(DATA_FILE);
        let app = app.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                return;
            }
            let names: Vec<String> =
                event.paths.iter().filter_map(|p| p.file_name()?.to_str().map(str::to_owned)).filter(|n| is_data_file(n)).collect();
            if names.is_empty() {
                return;
            }
            let conflict = names.iter().any(|n| n.to_lowercase().contains("conflict"));
            let contents = std::fs::read_to_string(&file).unwrap_or_default();
            if conflict || store::is_foreign_change(&contents) {
                store::invalidate();
                let _ = app.emit("store-changed", ());
            }
        })
        .map_err(AppError::other)?;
        watcher.watch(&dir, RecursiveMode::NonRecursive).map_err(AppError::other)?;
        *self.0.lock().unwrap() = Some(watcher);
        Ok(())
    }
}
