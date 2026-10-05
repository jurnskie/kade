//! Find the editors installed on this machine, for the editor picker.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct EditorOption {
    pub label: String,
    /// The command Kade runs with the file path appended.
    pub command: String,
    /// Runs inside a terminal window.
    pub terminal: bool,
}

pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// How to run a terminal editor in its own window, if we can.
fn terminal_prefix() -> Option<String> {
    if on_path("omarchy-launch-tui") {
        return Some("omarchy-launch-tui".into());
    }
    ["ghostty", "kitty", "alacritty", "wezterm", "foot", "konsole", "gnome-terminal"].into_iter().find(|t| on_path(t)).map(|t| match t {
        "wezterm" => "wezterm start --".into(),
        "gnome-terminal" => "gnome-terminal --".into(),
        t => format!("{t} -e"),
    })
}

/// The editor Omarchy is configured to use, if any.
pub fn omarchy_editor() -> Option<String> {
    if !on_path("omarchy-launch-editor") {
        return None;
    }
    let state = dirs::home_dir()?.join(".local/state/omarchy/defaults/editor");
    let name = std::fs::read_to_string(state).unwrap_or_else(|_| "nvim".into());
    Some(name.trim().rsplit('/').next().unwrap_or("nvim").to_string())
}

pub fn detect() -> Vec<EditorOption> {
    let mut out = Vec::new();
    let gui = |label: &str, command: &str| EditorOption { label: label.into(), command: command.into(), terminal: false };

    #[cfg(target_os = "macos")]
    {
        for app in ["Visual Studio Code", "Cursor", "Zed", "Sublime Text", "PhpStorm", "Nova", "BBEdit", "TextEdit"] {
            let installed =
                ["/Applications", "/System/Applications"].iter().any(|d| std::path::Path::new(d).join(format!("{app}.app")).exists());
            if installed {
                out.push(gui(app, &format!("open -a {}", shlex::try_quote(app).unwrap_or(app.into()))));
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        // (label, binary) — several distros name binaries differently.
        for (label, bin) in [
            ("Visual Studio Code", "code"),
            ("VSCodium", "codium"),
            ("Cursor", "cursor"),
            ("Zed", "zed"),
            ("Zed", "zeditor"),
            ("Sublime Text", "subl"),
            ("PhpStorm", "phpstorm"),
            ("Kate", "kate"),
            ("GNOME Text Editor", "gnome-text-editor"),
            ("gedit", "gedit"),
            ("Emacs", "emacs"),
        ] {
            if on_path(bin) && !out.iter().any(|o: &EditorOption| o.label == label) {
                out.push(gui(label, bin));
            }
        }
    }

    if let Some(prefix) = terminal_prefix() {
        for (label, bin) in [("Neovim", "nvim"), ("Vim", "vim"), ("Helix", "hx"), ("Micro", "micro"), ("Nano", "nano")] {
            if on_path(bin) {
                out.push(EditorOption { label: label.into(), command: format!("{prefix} {bin}"), terminal: true });
            }
        }
    }
    out
}
