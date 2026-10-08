//! Edit remote files in the user's own editor.
//!
//! The file is downloaded to a private temp folder and opened. A watcher on
//! that folder notices every save (editors often write a temp file and rename
//! it over the original, so the folder is watched, not the file). After a short
//! quiet period the new content is uploaded — unless the server copy changed
//! since we fetched it, in which case the user decides. Every upload first moves
//! the previous server version into a backup.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::backup::{Op, Recorder, Side};
use crate::error::{AppError, AppResult};
use crate::remote::{copy_chunks, no_op};
use crate::ssh::Session;

const QUIET: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EditState {
    Watching,
    Uploading,
    Uploaded,
    /// The server copy changed since we fetched it; waiting for the user.
    Conflict,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditInfo {
    pub id: String,
    pub session_id: String,
    pub name: String,
    pub remote_path: String,
    pub local_path: String,
    pub state: EditState,
    pub message: Option<String>,
    /// Unix ms of the last state change.
    pub at: i64,
    pub uploads: u32,
}

/// Emits progress to the frontend (an `edit` event in the app).
pub type Emit = Arc<dyn Fn(&EditInfo) + Send + Sync>;

/// What the server copy looked like when we last fetched or wrote it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Baseline {
    mtime: Option<i64>,
    size: u64,
}

struct Edit {
    session: Arc<Session>,
    info: Mutex<EditInfo>,
    baseline: Mutex<Baseline>,
    /// Hash of the content the server has, to skip saves that changed nothing.
    uploaded_hash: Mutex<u64>,
    /// Serialises uploads for this file.
    busy: tokio::sync::Mutex<()>,
    emit: Emit,
    _watcher: Mutex<Option<RecommendedWatcher>>,
}

#[derive(Default)]
pub struct Edits(Mutex<HashMap<String, Arc<Edit>>>);

fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Does the working copy hold something the server doesn't have? A conflict or
/// error means the last save never arrived, whatever the hashes say.
fn has_unsaved_changes(state: EditState, local_hash: Option<u64>, uploaded_hash: u64) -> bool {
    let Some(local_hash) = local_hash else { return false };
    matches!(state, EditState::Conflict | EditState::Error) || local_hash != uploaded_hash
}

fn edit_root() -> AppResult<PathBuf> {
    let dir = dirs::cache_dir().ok_or_else(|| AppError::other(tr!("No cache directory", "Geen cache-map")))?.join("kade/edit");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

impl Edit {
    fn set(&self, state: EditState, message: Option<String>) {
        let snapshot = {
            let mut info = self.info.lock().unwrap();
            info.state = state;
            info.message = message;
            info.at = crate::store::now_ms();
            if state == EditState::Uploaded {
                info.uploads += 1;
            }
            info.clone()
        };
        (self.emit)(&snapshot);
    }

    fn paths(&self) -> (String, PathBuf) {
        let info = self.info.lock().unwrap();
        (info.remote_path.clone(), PathBuf::from(&info.local_path))
    }

    async fn server_state(&self, remote: &str) -> AppResult<Baseline> {
        let meta = self.session.fs()?.stat(remote).await?;
        Ok(match meta {
            Some(m) => Baseline { mtime: m.mtime, size: m.size },
            None => Baseline { mtime: None, size: 0 },
        })
    }

    /// Download the server copy over the local working copy.
    async fn fetch(&self) -> AppResult<()> {
        let (remote, local) = self.paths();
        // Before reading: a change landing during the download then shows up as
        // a conflict on save, instead of being overwritten unnoticed.
        let baseline = self.server_state(&remote).await?;
        let fs = self.session.fs()?;
        let mut reader = fs.reader(&remote).await?;
        let mut content = Vec::new();
        copy_chunks(&mut reader, &mut content, no_op).await?;
        reader.finish().await?;
        let mut file = tokio::fs::File::create(&local).await?;
        file.write_all(&content).await?;
        file.flush().await?;
        *self.uploaded_hash.lock().unwrap() = hash(&content);
        *self.baseline.lock().unwrap() = baseline;
        Ok(())
    }

    /// Move unsaved changes of the working copy into a backup, before it is
    /// deleted or overwritten. Returns the backup id when something was kept.
    fn keep_unsaved(&self, op: Op) -> AppResult<Option<String>> {
        let (remote, local) = self.paths();
        let state = self.info.lock().unwrap().state;
        let local_hash = std::fs::read(&local).ok().map(|c| hash(&c));
        if !has_unsaved_changes(state, local_hash, *self.uploaded_hash.lock().unwrap()) {
            return Ok(None);
        }
        let undo = Recorder::new(
            Side::Local,
            op,
            None,
            tr!("Unsaved edit of {remote}", "Niet-opgeslagen bewerking van {remote}", remote = remote),
        );
        // A local stash never waits on anything, so it finishes on the first poll.
        undo.stash(None, &local.to_string_lossy()).now_or_never().ok_or_else(|| {
            AppError::other(tr!("Couldn't back up your unsaved changes", "Kon je niet-opgeslagen wijzigingen niet back-uppen"))
        })??;
        Ok(undo.commit()?.map(|tx| tx.id))
    }

    /// Upload the working copy. With `force`, ignore a changed server copy.
    async fn upload(&self, force: bool) -> AppResult<()> {
        let _busy = self.busy.lock().await;
        let (remote, local) = self.paths();
        let content = tokio::fs::read(&local).await?;
        let h = hash(&content);
        if h == *self.uploaded_hash.lock().unwrap() && !force {
            return Ok(()); // saved without changes
        }
        if !force {
            let now = self.server_state(&remote).await?;
            if now != *self.baseline.lock().unwrap() {
                self.set(
                    EditState::Conflict,
                    Some(tr!(
                        "The file on the server changed since you opened it",
                        "Het bestand op de server is gewijzigd sinds je het opende"
                    )),
                );
                return Ok(());
            }
        }
        self.set(EditState::Uploading, None);

        let fs = self.session.fs()?;
        // Write through a symlink instead of replacing it, and give the new file
        // the mode and owner of the one it replaces.
        let (target, kept) = fs.overwrite_target(&remote).await?;
        // Keep the previous server version, as with every overwrite.
        let undo =
            Recorder::new(Side::Remote, Op::Overwrite, Some(&self.session), tr!("Edited: {remote}", "Bewerkt: {remote}", remote = remote));
        let replaces = fs.exists(&target).await?;
        if replaces {
            undo.stash(Some(&self.session), &target).await?;
        }
        let result = async {
            let mut writer = fs.writer(&target).await?;
            writer.write_all(&content).await?;
            writer.finish().await
        }
        .await;
        if let Err(e) = result {
            // Never leave a live site without the file: put the old version back.
            let mut restore_failed = None;
            if replaces {
                let _ = fs.remove_file(&target).await; // the half-written file
                restore_failed = undo.put_back(Some(&self.session), &target).await.err();
            }
            let tx = undo.commit()?;
            return Err(match (restore_failed, tx) {
                (Some(err), Some(tx)) => AppError::other(tr!(
                    "{e}. Putting the previous version back failed ({err}); it is kept in backup {id}",
                    "{e}. De vorige versie terugzetten is mislukt ({err}); die staat in backup {id}",
                    e = e,
                    err = err,
                    id = tx.id
                )),
                _ => e,
            });
        }
        undo.commit()?;
        if let Some(kept) = &kept {
            fs.restore_kept(&target, kept).await;
        }

        *self.uploaded_hash.lock().unwrap() = h;
        *self.baseline.lock().unwrap() = self.server_state(&remote).await?;
        self.set(EditState::Uploaded, None);
        Ok(())
    }
}

/// Start the user's editor on `path`. An empty command uses Omarchy's editor
/// launcher when present, else the system's default app; `{file}` in the command is replaced, otherwise the path is
/// appended (e.g. `code`, `zed`, `ghostty -e nvim`).
pub fn launch(editor: &str, path: &Path) -> AppResult<()> {
    let editor = editor.trim();
    if editor.is_empty() {
        // Omarchy knows the user's editor and how to open it (TUI or GUI).
        if crate::editors::on_path("omarchy-launch-editor") {
            return launch("omarchy-launch-editor", path);
        }
        // xdg-open directly rather than via the opener plugin, so it gets a
        // clean environment when Kade runs as an AppImage.
        if cfg!(target_os = "linux") {
            return launch("xdg-open", path);
        }
        return tauri_plugin_opener::open_path(path, None::<&str>).map_err(AppError::other);
    }
    let mut parts = shlex::split(editor).ok_or_else(|| AppError::other(tr!("Invalid editor command", "Ongeldige editor-opdracht")))?;
    let file = path.to_string_lossy().into_owned();
    if parts.iter().any(|p| p.contains("{file}")) {
        for p in &mut parts {
            *p = p.replace("{file}", &file);
        }
    } else {
        parts.push(file);
    }
    let (program, args) = parts.split_first().ok_or_else(|| AppError::other(tr!("Empty editor command", "Lege editor-opdracht")))?;
    crate::hostenv::command(program)
        .args(args)
        .spawn()
        .map_err(|e| AppError::other(tr!("Can't start {program}: {e}", "Kan {program} niet starten: {e}", program = program, e = e)))?;
    Ok(())
}

impl Edits {
    fn get(&self, id: &str) -> AppResult<Arc<Edit>> {
        self.0.lock().unwrap().get(id).cloned().ok_or_else(|| AppError::other(tr!("Edit not found", "Bewerking niet gevonden")))
    }

    pub fn list(&self) -> Vec<EditInfo> {
        self.0.lock().unwrap().values().map(|e| e.info.lock().unwrap().clone()).collect()
    }

    /// Fetch `remote_path`, open it in the editor and upload every save.
    /// Opening a file that is already being edited just reopens the editor.
    pub async fn open(
        &self,
        emit: Emit,
        session: Arc<Session>,
        session_id: String,
        remote_path: String,
        editor: &str,
    ) -> AppResult<EditInfo> {
        let existing = self
            .0
            .lock()
            .unwrap()
            .values()
            .find(|e| {
                let i = e.info.lock().unwrap();
                i.session_id == session_id && i.remote_path == remote_path
            })
            .cloned();
        if let Some(edit) = existing {
            let (_, local) = edit.paths();
            launch(editor, &local)?;
            return Ok(edit.info.lock().unwrap().clone());
        }

        let id = uuid::Uuid::new_v4().to_string();
        let name = remote_path.rsplit('/').next().unwrap_or(&remote_path).to_string();
        // One folder per edit keeps the real file name (editors pick syntax from it).
        let dir = edit_root()?.join(&id);
        std::fs::create_dir_all(&dir)?;
        let local = dir.join(&name);

        let edit = Arc::new(Edit {
            session,
            info: Mutex::new(EditInfo {
                id: id.clone(),
                session_id,
                name: name.clone(),
                remote_path,
                local_path: local.to_string_lossy().into_owned(),
                state: EditState::Watching,
                message: None,
                at: crate::store::now_ms(),
                uploads: 0,
            }),
            baseline: Mutex::new(Baseline { mtime: None, size: 0 }),
            uploaded_hash: Mutex::new(0),
            busy: tokio::sync::Mutex::new(()),
            emit,
            _watcher: Mutex::new(None),
        });
        edit.fetch().await?;

        // Watch the folder; debounce bursts of events into one upload.
        let (tx, mut rx) = mpsc::unbounded_channel::<()>();
        let file_name = std::ffi::OsString::from(&name);
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let ours = ev.paths.iter().any(|p| p.file_name() == Some(file_name.as_os_str()));
                let writes = matches!(ev.kind, notify::EventKind::Modify(_) | notify::EventKind::Create(_));
                if ours && writes {
                    let _ = tx.send(());
                }
            }
        })
        .map_err(AppError::other)?;
        watcher.watch(&dir, RecursiveMode::NonRecursive).map_err(AppError::other)?;
        *edit._watcher.lock().unwrap() = Some(watcher);

        let weak = Arc::downgrade(&edit);
        tauri::async_runtime::spawn(async move {
            while rx.recv().await.is_some() {
                // Wait until the editor has been quiet for a moment.
                while tokio::time::timeout(QUIET, rx.recv()).await.is_ok_and(|m| m.is_some()) {}
                let Some(edit) = weak.upgrade() else { break };
                if let Err(e) = edit.upload(false).await {
                    edit.set(EditState::Error, Some(e.to_string()));
                }
            }
        });

        self.0.lock().unwrap().insert(id, edit.clone());
        launch(editor, &local)?;
        let info = edit.info.lock().unwrap().clone();
        (edit.emit)(&info);
        Ok(info)
    }

    /// Resolve a conflict by uploading anyway.
    pub async fn force_upload(&self, id: &str) -> AppResult<()> {
        let edit = self.get(id)?;
        if let Err(e) = edit.upload(true).await {
            edit.set(EditState::Error, Some(e.to_string()));
            return Err(e);
        }
        Ok(())
    }

    /// Resolve a conflict by taking the server version (the editor reloads it).
    pub async fn reload(&self, id: &str) -> AppResult<()> {
        let edit = self.get(id)?;
        let _busy = edit.busy.lock().await;
        // Fetching overwrites the working copy; keep what the user typed.
        let kept = edit.keep_unsaved(Op::Overwrite)?;
        edit.fetch().await?;
        let message = match kept {
            Some(id) => tr!(
                "Fetched the server version; your changes are kept in backup {id}",
                "Serverversie opgehaald; je wijzigingen staan in backup {id}",
                id = id
            ),
            None => tr!("Fetched the server version", "Serverversie opgehaald"),
        };
        edit.set(EditState::Watching, Some(message));
        Ok(())
    }

    /// Stop watching and remove the working copy.
    pub fn stop(&self, id: &str) {
        let removed = self.0.lock().unwrap().remove(id);
        if let Some(edit) = removed {
            // Unsaved changes go to a backup first; if that fails, the folder stays.
            if let Err(e) = edit.keep_unsaved(Op::Delete) {
                eprintln!("kade: keeping the unsaved edit of {} failed, leaving it in place: {e}", edit.paths().0);
                return;
            }
            let (_, local) = edit.paths();
            if let Some(dir) = local.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    pub fn stop_session(&self, session_id: &str) {
        let ids: Vec<String> = self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, e)| e.info.lock().unwrap().session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.stop(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{has_unsaved_changes, EditState};

    #[test]
    fn unsaved_changes_are_detected() {
        use EditState::*;
        assert!(!has_unsaved_changes(Watching, Some(1), 1), "saved and identical");
        assert!(has_unsaved_changes(Watching, Some(2), 1), "saved locally, not uploaded");
        assert!(has_unsaved_changes(Conflict, Some(1), 1), "conflict keeps the file");
        assert!(has_unsaved_changes(Error, Some(1), 1), "failed upload keeps the file");
        assert!(!has_unsaved_changes(Error, None, 1), "nothing left to keep");
    }
}
