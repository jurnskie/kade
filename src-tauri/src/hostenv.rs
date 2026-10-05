//! Start other programs (curl, op, editors, xdg-open) with the user's own
//! environment.
//!
//! The AppImage's AppRun points LD_LIBRARY_PATH, PATH, GTK_PATH, GIO_*,
//! PYTHONHOME and friends into its mount (`$APPDIR`). Kade itself needs that
//! (WebKit starts helper processes), but system programs inheriting it load
//! the bundled libraries and fail, e.g. `curl: symbol lookup error`. So every
//! child gets those entries stripped. Outside an AppImage this changes nothing.

use std::ffi::{OsStr, OsString};

/// Variables AppRun sets that mean nothing to other programs.
const APPRUN_ONLY: &[&str] = &["APPDIR", "APPIMAGE", "ARGV0", "OWD", "PYTHONDONTWRITEBYTECODE"];

pub fn command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    if let Some(appdir) = appdir() {
        for (key, value) in cleaned(std::env::vars_os(), &appdir) {
            match value {
                Some(v) => cmd.env(key, v),
                None => cmd.env_remove(key),
            };
        }
    }
    cmd
}

pub fn async_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    command(program).into()
}

fn appdir() -> Option<String> {
    std::env::var_os("APPIMAGE")?;
    std::env::var("APPDIR").ok().filter(|d| d.len() > 1)
}

/// The changes to make: `None` removes a variable, `Some` replaces it with
/// its value minus every path inside `appdir`.
fn cleaned(vars: impl Iterator<Item = (OsString, OsString)>, appdir: &str) -> Vec<(OsString, Option<OsString>)> {
    let appdir = appdir.trim_end_matches('/');
    let inside = |p: &str| p == appdir || p.starts_with(&format!("{appdir}/"));
    vars.filter_map(|(key, value)| {
        if APPRUN_ONLY.iter().any(|k| key == *k) {
            return Some((key, None));
        }
        let value = value.to_str()?;
        if !value.contains(appdir) {
            return None;
        }
        let kept: Vec<&str> = value.split(':').filter(|p| !p.is_empty() && !inside(p)).collect();
        Some((key, (!kept.is_empty()).then(|| kept.join(":").into())))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_appimage_paths() {
        let d = "/tmp/.mount_kadeAB12";
        let vars = [
            ("LD_LIBRARY_PATH", format!("{d}/usr/lib/:{d}/lib64/:")),
            ("PATH", format!("{d}/usr/bin/:/usr/local/bin:/usr/bin")),
            ("XDG_DATA_DIRS", format!("{d}/usr/share/:/usr/share")),
            ("GTK_EXE_PREFIX", format!("{d}//usr")),
            ("APPDIR", d.to_string()),
            ("HOME", "/home/me".to_string()),
            ("OTHER", "/tmp/.mount_kadeAB12x/not-ours".to_string()),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let out: std::collections::HashMap<String, Option<OsString>> =
            cleaned(vars.into_iter(), d).into_iter().map(|(k, v)| (k.into_string().unwrap(), v)).collect();
        assert_eq!(out["LD_LIBRARY_PATH"], None);
        assert_eq!(out["PATH"], Some("/usr/local/bin:/usr/bin".into()));
        assert_eq!(out["XDG_DATA_DIRS"], Some("/usr/share".into()));
        assert_eq!(out["GTK_EXE_PREFIX"], None);
        assert_eq!(out["APPDIR"], None);
        assert!(!out.contains_key("HOME"));
        assert_eq!(out["OTHER"], Some("/tmp/.mount_kadeAB12x/not-ours".into()));
    }
}
