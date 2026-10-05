use std::os::unix::fs::PermissionsExt;
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::error::{AppError, AppResult};

#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    /// Unix seconds.
    pub modified: Option<i64>,
    /// `drwxr-xr-x` style.
    pub permissions: Option<String>,
}

pub fn format_mode(mode: u32) -> String {
    let kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        _ => '-',
    };
    let bits = ['r', 'w', 'x'];
    let mut s = String::with_capacity(10);
    s.push(kind);
    for i in (0..9).rev() {
        s.push(if mode & (1 << i) != 0 { bits[(8 - i) % 3] } else { '-' });
    }
    s
}

pub fn home() -> String {
    dirs::home_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into())
}

pub fn list(path: &str) -> AppResult<Vec<Entry>> {
    let mut out = Vec::new();
    for item in std::fs::read_dir(path)? {
        let item = item?;
        let link_meta = item.metadata()?;
        let is_symlink = link_meta.file_type().is_symlink();
        // Follow symlinks for size/dir-ness; fall back to the link itself if it dangles.
        let meta = std::fs::metadata(item.path()).unwrap_or(link_meta);
        out.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            path: item.path().to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
            is_symlink,
            size: meta.len(),
            modified: meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64),
            permissions: Some(format_mode(meta.permissions().mode())),
        });
    }
    Ok(out)
}

/// Refuse operations on paths that would be catastrophic to get wrong.
pub fn guard(path: &str) -> AppResult<()> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "~" || Some(trimmed.to_string()) == dirs::home_dir().map(|h| h.to_string_lossy().into_owned()) {
        return Err(AppError::other(tr!("Refusing to operate on {path}", "Weigert bewerking op {path}", path = path)));
    }
    Ok(())
}

pub fn mkdir(path: &str) -> AppResult<()> {
    Ok(std::fs::create_dir(path)?)
}

pub fn create_file(path: &str) -> AppResult<()> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    Ok(())
}

pub fn rename(from: &str, to: &str) -> AppResult<()> {
    if std::path::Path::new(to).exists() {
        return Err(AppError::other(format!("{to} bestaat al")));
    }
    Ok(std::fs::rename(from, to)?)
}

/// Delete a file, symlink or directory tree. Symlinks are removed, never followed.
pub fn delete(path: &str) -> AppResult<()> {
    guard(path)?;
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::format_mode;

    #[test]
    fn guards_dangerous_paths() {
        assert!(super::guard("/").is_err());
        assert!(super::guard("").is_err());
        assert!(super::guard("/tmp/x").is_ok());
    }

    #[test]
    fn formats_modes() {
        assert_eq!(format_mode(0o040755), "drwxr-xr-x");
        assert_eq!(format_mode(0o100600), "-rw-------");
        assert_eq!(format_mode(0o120777), "lrwxrwxrwx");
    }
}
