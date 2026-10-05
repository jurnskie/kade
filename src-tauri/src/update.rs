//! Update Kade from its public GitHub releases, over HTTPS with `curl`.
//!
//! Only installed builds can replace themselves: the Linux AppImage (its path
//! is in $APPIMAGE) and the macOS Kade.app bundle. A dev build says so.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::process::Command;

use crate::error::{AppError, AppResult};

const REPO: &str = "jurnskie/kade";

/// macOS and Linux release assets; the names are the same in every release.
pub const MAC_ASSET: &str = "Kade-macos-universal.zip";
pub const LINUX_ASSET: &str = "Kade-linux-x86_64.AppImage";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallKind {
    AppImage {
        path: String,
    },
    MacApp {
        path: String,
    },
    /// Running from a source checkout or package manager: no self-update.
    Other {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    pub newer: bool,
    pub notes: Option<String>,
    pub install: InstallKind,
    /// Why checking failed (offline, rate-limited, …).
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    body: Option<String>,
}

fn install_kind() -> InstallKind {
    if let Ok(path) = std::env::var("APPIMAGE") {
        return InstallKind::AppImage { path };
    }
    let exe = std::env::current_exe().unwrap_or_default();
    // …/Kade.app/Contents/MacOS/kade
    if let Some(bundle) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
        if cfg!(target_os = "macos") {
            return InstallKind::MacApp { path: bundle.to_string_lossy().into_owned() };
        }
    }
    let reason = if cfg!(debug_assertions) || exe.components().any(|c| c.as_os_str() == "target") {
        tr!("This is a development build; update with git pull.", "Dit is een ontwikkelversie; werk bij met git pull.")
    } else {
        tr!(
            "This installation can't update itself; use the install script.",
            "Deze installatie kan zichzelf niet bijwerken; gebruik het installatiescript."
        )
    };
    InstallKind::Other { reason }
}

/// Compare dotted versions numerically ("0.10.0" > "0.9.3").
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    parse(latest) > parse(current)
}

async fn curl(args: &[&str]) -> AppResult<String> {
    let out =
        Command::new("curl").args(["-fsSL", "--proto", "=https", "--retry", "2", "-A", "kade-updater"]).args(args).output().await.map_err(
            |_| {
                AppError::other(tr!(
                    "curl not found; install it to check for updates",
                    "curl niet gevonden; installeer die om op updates te controleren"
                ))
            },
        )?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(AppError::other(tr!("Can't reach GitHub: {e}", "GitHub is niet bereikbaar: {e}", e = err)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub async fn check(current: &str) -> UpdateInfo {
    let install = install_kind();
    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    match curl(&["-H", "Accept: application/vnd.github+json", &api]).await {
        Ok(raw) => match serde_json::from_str::<Release>(&raw) {
            Ok(r) => {
                let latest = r.tag_name.trim_start_matches('v').to_string();
                UpdateInfo {
                    current: current.into(),
                    newer: is_newer(&latest, current),
                    latest: Some(latest),
                    notes: r.body.filter(|b| !b.trim().is_empty()),
                    install,
                    error: None,
                }
            }
            Err(e) => UpdateInfo { current: current.into(), latest: None, newer: false, notes: None, install, error: Some(e.to_string()) },
        },
        Err(e) => UpdateInfo { current: current.into(), latest: None, newer: false, notes: None, install, error: Some(e.to_string()) },
    }
}

fn progress(app: &AppHandle, msg: &str) {
    let _ = app.emit("update-progress", msg);
}

/// Download `asset` (plus its .sha256) from release `tag` and verify it.
async fn download_verified(tag: &str, asset: &str, dir: &Path) -> AppResult<PathBuf> {
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");
    let file = dir.join(asset);
    let sum = dir.join(format!("{asset}.sha256"));
    curl(&["-o", &sum.to_string_lossy(), &format!("{base}/{asset}.sha256")]).await?;
    curl(&["-o", &file.to_string_lossy(), &format!("{base}/{asset}")]).await?;
    let expected = std::fs::read_to_string(&sum)?;
    let expected = expected.split_whitespace().next().unwrap_or_default().to_lowercase();
    let actual = hex::encode(Sha256::digest(std::fs::read(&file)?));
    if expected != actual {
        return Err(AppError::other(tr!("Checksum mismatch; update not installed", "Checksum klopt niet; update niet geïnstalleerd")));
    }
    Ok(file)
}

/// Install release `version` over the running app, then restart it.
pub async fn install(app: AppHandle, version: String) -> AppResult<()> {
    let tag = format!("v{}", version.trim_start_matches('v'));
    let work = std::env::temp_dir().join(format!("kade-update-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&work)?;
    let result = async {
        match install_kind() {
            InstallKind::AppImage { path } => {
                progress(&app, &tr!("Downloading…", "Downloaden…"));
                let image = download_verified(&tag, LINUX_ASSET, &work).await?;
                progress(&app, &tr!("Installing…", "Installeren…"));
                // Write next to the target and rename: atomic, and the running
                // process keeps its (now unlinked) old file.
                let target = PathBuf::from(&path);
                let staged = target.with_extension("new");
                std::fs::copy(&image, &staged)?;
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
                std::fs::rename(&staged, &target)?;
                if let Some(dir) = target.parent() {
                    let marker = dir.join(".kade-version");
                    if marker.exists() {
                        let _ = std::fs::write(marker, format!("{}\n", tag.trim_start_matches('v')));
                    }
                }
                progress(&app, &tr!("Restarting…", "Herstarten…"));
                std::process::Command::new(&target).spawn()?;
                Ok(())
            }
            InstallKind::MacApp { path } => {
                progress(&app, &tr!("Downloading…", "Downloaden…"));
                let zip = download_verified(&tag, MAC_ASSET, &work).await?;
                progress(&app, &tr!("Installing…", "Installeren…"));
                let unpacked = work.join("unpacked");
                let status = Command::new("ditto").args(["-x", "-k"]).arg(&zip).arg(&unpacked).status().await?;
                if !status.success() {
                    return Err(AppError::other(tr!("Unpacking failed", "Uitpakken mislukt")));
                }
                let new_app = unpacked.join("Kade.app");
                let target = PathBuf::from(&path);
                let staged = target.with_extension("app.new");
                let old = target.with_extension("app.old");
                let _ = std::fs::remove_dir_all(&staged);
                let _ = std::fs::remove_dir_all(&old);
                let status = Command::new("ditto").arg(&new_app).arg(&staged).status().await?;
                if !status.success() {
                    return Err(AppError::other(tr!("Copying failed", "Kopiëren mislukt")));
                }
                let _ = Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(&staged).status().await;
                std::fs::rename(&target, &old)?;
                if let Err(e) = std::fs::rename(&staged, &target) {
                    let _ = std::fs::rename(&old, &target); // put the working version back
                    return Err(e.into());
                }
                let _ = std::fs::remove_dir_all(&old);
                progress(&app, &tr!("Restarting…", "Herstarten…"));
                // Start the new bundle once this process has exited.
                std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(format!("sleep 1; open -n {}", shlex::try_quote(&path).map_err(AppError::other)?))
                    .spawn()?;
                Ok(())
            }
            InstallKind::Other { reason } => Err(AppError::other(reason)),
        }
    }
    .await;
    let _ = std::fs::remove_dir_all(&work);
    result?;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run alone (it changes PATH): cargo test download_verified -- --ignored
    #[tokio::test]
    #[ignore]
    async fn download_verified_checks_the_checksum() {
        let tmp = std::env::temp_dir().join(format!("kade-upd-{}", uuid::Uuid::new_v4()));
        let (bin, rel) = (tmp.join("bin"), tmp.join("rel"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&rel).unwrap();
        std::fs::write(rel.join(LINUX_ASSET), b"new version").unwrap();
        let sum = hex::encode(Sha256::digest(b"new version"));
        std::fs::write(rel.join(format!("{LINUX_ASSET}.sha256")), format!("{sum}  {LINUX_ASSET}\n")).unwrap();
        // Fake curl: `… -o <file> <url>` copies the release file named at the end of the URL.
        let curl = bin.join("curl");
        std::fs::write(
            &curl,
            format!("#!/bin/sh\nwhile [ $# -gt 1 ]; do [ \"$1\" = -o ] && o=$2; shift; done\ncp {}/\"${{1##*/}}\" \"$o\"\n", rel.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));

        let dir = tmp.join("dl1");
        std::fs::create_dir_all(&dir).unwrap();
        let file = download_verified("v9.9.9", LINUX_ASSET, &dir).await.unwrap();
        assert_eq!(std::fs::read(file).unwrap(), b"new version");

        // Tampered download is refused.
        std::fs::write(rel.join(LINUX_ASSET), b"something else").unwrap();
        let dir = tmp.join("dl2");
        std::fs::create_dir_all(&dir).unwrap();
        let err = download_verified("v9.9.9", LINUX_ASSET, &dir).await.unwrap_err();
        assert!(err.to_string().contains("Checksum"));
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
    }
}
