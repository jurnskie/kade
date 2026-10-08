//! Finding the Laravel project around a folder, for the Sync dialog's
//! "Content", "Assets" and "storage/app" shortcuts. A project is a folder
//! with an `artisan` file.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::remote::RemoteFs;

/// How far up the server walk goes; real projects sit a handful of levels deep.
const MAX_REMOTE_DEPTH: usize = 12;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LocalProject {
    pub root: String,
    pub content: bool,
    pub assets: bool,
    pub storage: bool,
}

/// The nearest folder at or above `start` that has an `artisan` file, never
/// looking at `stop` (the home folder) or above it, nor at `/`.
fn find_root(start: &Path, stop: Option<&Path>) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.parent().is_none() || Some(dir) == stop {
            return None;
        }
        if dir.join("artisan").is_file() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

fn project_at(root: &Path) -> LocalProject {
    LocalProject {
        root: root.to_string_lossy().into_owned(),
        content: root.join("content").is_dir(),
        assets: root.join("public/assets").is_dir(),
        storage: root.join("storage/app").is_dir(),
    }
}

pub fn local_project(path: &str) -> Option<LocalProject> {
    let home = dirs::home_dir();
    find_root(Path::new(path), home.as_deref()).map(|r| project_at(&r))
}

pub fn local_realpath(path: &str) -> AppResult<String> {
    let real = std::fs::canonicalize(path)
        .map_err(|e| AppError::other(tr!("Can't resolve {path}: {err}", "Kan {path} niet omzetten: {err}", path = path, err = e)))?;
    Ok(real.to_string_lossy().into_owned())
}

/// Like [`find_root`], on the server. `stat` follows symlinks, so a Deployer
/// `current` link counts as the release it points to.
pub async fn remote_root(fs: &RemoteFs, path: &str) -> AppResult<Option<String>> {
    let mut dir = path.trim_end_matches('/').to_string();
    for _ in 0..MAX_REMOTE_DEPTH {
        if dir.is_empty() {
            return Ok(None);
        }
        if fs.stat(&format!("{dir}/artisan")).await?.is_some_and(|m| !m.is_dir) {
            return Ok(Some(dir));
        }
        dir.truncate(dir.rfind('/').unwrap_or(0));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kade-laravel-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // macOS puts temp dirs behind /var -> /private/var.
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn finds_the_project_from_a_subfolder() {
        let base = temp("walk");
        let root = base.join("home/site");
        std::fs::create_dir_all(root.join("public/assets/img")).unwrap();
        std::fs::write(root.join("artisan"), "").unwrap();
        let found = find_root(&root.join("public/assets/img"), Some(&base.join("home")));
        assert_eq!(found, Some(root.clone()));
        assert_eq!(find_root(&root, None), Some(root));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn stops_at_the_home_folder() {
        let base = temp("home");
        let home = base.join("home");
        std::fs::create_dir_all(home.join("docs")).unwrap();
        std::fs::write(home.join("artisan"), "").unwrap();
        assert_eq!(find_root(&home.join("docs"), Some(&home)), None);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_folder_called_artisan_is_not_a_project() {
        let base = temp("dir");
        std::fs::create_dir_all(base.join("a/artisan")).unwrap();
        assert_eq!(find_root(&base.join("a"), Some(&base)), None);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn reports_which_shortcut_folders_exist() {
        let base = temp("shortcuts");
        std::fs::create_dir_all(base.join("content")).unwrap();
        std::fs::create_dir_all(base.join("public/assets")).unwrap();
        std::fs::write(base.join("artisan"), "").unwrap();
        let p = project_at(&base);
        assert_eq!((p.content, p.assets, p.storage), (true, true, false));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn realpath_follows_symlinks_like_a_deployer_layout() {
        let base = temp("deployer");
        std::fs::create_dir_all(base.join("shared/public/assets")).unwrap();
        std::fs::create_dir_all(base.join("releases/1/public")).unwrap();
        std::fs::write(base.join("releases/1/artisan"), "").unwrap();
        symlink(base.join("shared/public/assets"), base.join("releases/1/public/assets")).unwrap();
        symlink(base.join("releases/1"), base.join("current")).unwrap();

        let via_current = base.join("current/public/assets");
        assert_eq!(local_realpath(via_current.to_str().unwrap()).unwrap(), base.join("shared/public/assets").to_str().unwrap());
        // The project is found through the `current` symlink too.
        assert_eq!(find_root(&via_current, Some(&base)), Some(base.join("current")));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn realpath_of_a_missing_folder_is_an_error() {
        assert!(local_realpath("/definitely/not/here").is_err());
    }
}
