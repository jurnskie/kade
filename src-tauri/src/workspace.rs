//! Creating, renaming and deleting workspaces.

use crate::error::{AppError, AppResult};
use crate::store::{self, Workspace, DEFAULT_WORKSPACE};

const COLORS: [&str; 6] = ["pine", "blue", "amber", "plum", "coral", "slate"];

pub fn save(ws: &mut Workspace) -> AppResult<()> {
    ws.name = ws.name.trim().to_string();
    if ws.name.is_empty() {
        return Err(AppError::other(tr!("A workspace needs a name", "Een workspace heeft een naam nodig")));
    }
    if !COLORS.contains(&ws.color.as_str()) {
        ws.color = "pine".into();
    }
    if ws.id.is_empty() {
        ws.id = uuid::Uuid::new_v4().to_string();
    }
    let clean = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    ws.op_account = clean(ws.op_account.take());
    ws.op_vault = clean(ws.op_vault.take());
    // A default key only makes sense inside its vault.
    ws.op_key_fingerprint = clean(ws.op_key_fingerprint.take()).filter(|_| ws.op_vault.is_some());
    ws.op_key_item = clean(ws.op_key_item.take()).filter(|_| ws.op_vault.is_some());
    ws.op_key_title = clean(ws.op_key_title.take()).filter(|_| ws.op_key_fingerprint.is_some());
    ws.updated_at = store::now_ms();
    let saved = ws.clone();
    store::update(|data| {
        // Materialise the implicit default first, so it isn't lost when the
        // list stops being empty.
        if data.workspaces.is_empty() && saved.id != DEFAULT_WORKSPACE {
            data.workspaces.push(Workspace::fallback());
        }
        if data.workspaces.iter().any(|w| w.id != saved.id && w.name.eq_ignore_ascii_case(&saved.name)) {
            return Err(AppError::other(tr!("There already is a workspace '{}'", "Er is al een workspace '{}'", saved.name)));
        }
        data.deleted.remove(&saved.id);
        match data.workspaces.iter_mut().find(|w| w.id == saved.id) {
            Some(w) => *w = saved.clone(),
            None => data.workspaces.push(saved.clone()),
        }
        Ok(())
    })
}

pub fn delete(id: &str, move_to: &str) -> AppResult<()> {
    if id == move_to {
        return Err(AppError::other(tr!(
            "Pick another workspace to move the connections to",
            "Kies een andere workspace om de verbindingen naartoe te verplaatsen"
        )));
    }
    store::update(|data| {
        let all = data.workspaces_or_default();
        if all.len() <= 1 {
            return Err(AppError::other(tr!("The last workspace can't be deleted", "De laatste workspace kan niet weg")));
        }
        if !all.iter().any(|w| w.id == move_to) {
            return Err(AppError::other(tr!("Unknown workspace to move to", "Onbekende workspace om naartoe te verplaatsen")));
        }
        if data.workspaces.is_empty() {
            data.workspaces = all.clone();
        }
        let now = store::now_ms();
        let is_default = id == DEFAULT_WORKSPACE;
        for s in data.servers.iter_mut() {
            if s.workspace == id || (is_default && s.workspace.is_empty()) {
                s.workspace = move_to.to_string();
                s.updated_at = now;
            }
        }
        data.workspaces.retain(|w| w.id != id);
        data.deleted.insert(id.to_string(), now);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{self, Auth, Protocol, ServerProfile};

    /// Create a workspace, put a connection in it, delete the workspace.
    #[test]
    fn create_move_and_delete() {
        let tmp = std::env::temp_dir().join(format!("kade-ws-{}", uuid::Uuid::new_v4()));
        let _env = crate::backup::TEST_ENV.blocking_lock();
        std::env::set_var("KADE_CONFIG_HOME", &tmp);

        // Fresh store: only the implicit default.
        let ws = store::load().unwrap().workspaces_or_default();
        let home = Workspace::fallback().name;
        assert_eq!((ws.len(), ws[0].name.as_str()), (1, home.as_str()));
        assert!(delete(DEFAULT_WORKSPACE, "x").is_err(), "the last workspace must stay");

        let mut werk = Workspace {
            id: String::new(),
            name: "Werk".into(),
            color: "blue".into(),
            op_account: Some("ACC2".into()),
            op_vault: Some(" ".into()),
            op_key_fingerprint: Some("SHA256:abc".into()),
            op_key_item: Some("op://V/I".into()),
            op_key_title: Some("Deploy".into()),
            updated_at: 0,
        };
        save(&mut werk).unwrap();
        let ws = store::load().unwrap().workspaces_or_default();
        assert_eq!(ws.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), [home.as_str(), "Werk"]);
        // A blank vault is none, and a default key does not outlive its vault.
        assert_eq!((ws[1].op_vault.as_deref(), ws[1].op_key_fingerprint.as_deref(), ws[1].op_key_item.as_deref()), (None, None, None));
        assert_eq!(ws[1].op_key_title, None);
        werk.op_vault = Some("V".into());
        werk.op_key_fingerprint = Some("SHA256:abc".into());
        werk.op_key_item = Some("op://V/I".into());
        werk.op_key_title = Some("Deploy".into());
        save(&mut werk).unwrap();
        let ws = store::load().unwrap().workspaces_or_default();
        assert_eq!(ws[1].op_key_fingerprint.as_deref(), Some("SHA256:abc"));
        assert_eq!(ws[1].op_key_title.as_deref(), Some("Deploy"));

        let mut dup = Workspace { name: "werk".into(), ..Workspace::fallback() };
        dup.id = String::new();
        assert!(save(&mut dup).is_err(), "names are unique");

        let conn = profiles::upsert(ServerProfile {
            id: String::new(),
            name: "prod-01".into(),
            protocol: Protocol::Ssh,
            host: "prod".into(),
            port: 22,
            user: "deploy".into(),
            group: String::new(),
            auth: Auth::OnePassword { key_fingerprint: None, account: None, key_item: None },
            remote_path: None,
            local_path: None,
            workspace: werk.id.clone(),
            tunnels: Vec::new(),
            updated_at: 0,
        })
        .unwrap();
        let data = store::load().unwrap();
        assert_eq!(data.workspace_of(&profiles::get(&conn.id).unwrap()).unwrap().op_account.as_deref(), Some("ACC2"));

        delete(&werk.id, DEFAULT_WORKSPACE).unwrap();
        let data = store::load().unwrap();
        assert_eq!(data.workspaces_or_default().len(), 1);
        assert_eq!(profiles::get(&conn.id).unwrap().workspace, DEFAULT_WORKSPACE);

        std::fs::remove_dir_all(tmp).ok();
    }
}
