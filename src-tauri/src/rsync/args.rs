//! rsync's command line and exclude file, built from a sync spec.
//!
//! Only flags that openrsync, 2.6.9 and GNU 3.x all understand. No `-p` and no
//! `--executability`: an existing file keeps its mode, a new one gets the
//! source's (minus the umask), like Kade's own transfers. (`--executability`
//! would strip the x bit from an executable the source has without it.) Paths
//! are absolute, so they can't be read as options or as `host:path`.

use std::ffi::OsString;
use std::path::Path;

use super::{Direction, SyncRequest};
use crate::error::{AppError, AppResult};

/// The "host" rsync hands to the relay; the bridge never resolves it.
pub const HOST: &str = "kade";
/// Stands in for the remote path on rsync's command line. The bridge puts the
/// real, quoted path from the job spec in its place.
pub const REMOTE: &str = "KADE_REMOTE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Dry run that lists what a sync would do.
    Preview,
    /// The real transfer; replaced files go to `--backup-dir`.
    Run,
    /// Dry run after the transfer that lists what is left to delete.
    Deletions,
}

pub fn build(spec: &SyncRequest, pass: Pass, backup_dir: Option<&str>, rsh: &Path, excludes: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-r", "-l", "-t", "-K", "-i", "-8", "--out-format=%i %l %n", "--timeout=120"].map(OsString::from).into();
    let mut exclude_from = OsString::from("--exclude-from=");
    exclude_from.push(excludes);
    args.push(exclude_from);
    if spec.checksum {
        args.push("-c".into());
    }
    match pass {
        Pass::Preview => {
            args.push("-n".into());
            if spec.delete {
                args.push("--delete-after".into());
            }
        }
        // Deletes are never left to rsync: openrsync skips them silently when
        // `--backup-dir` is set. Kade moves them into the backup itself.
        Pass::Run => {
            if let Some(dir) = backup_dir {
                args.push("-b".into());
                args.push(format!("--backup-dir={dir}").into());
            }
        }
        Pass::Deletions => args.extend(["-n", "--delete-after"].map(OsString::from)),
    }
    args.push("-e".into());
    args.push(rsh.into());
    // A trailing slash on the source copies its contents into the destination.
    let remote = format!("{HOST}:{REMOTE}");
    match spec.direction {
        Direction::ToServer => {
            args.push(format!("{}/", spec.local.trim_end_matches('/')).into());
            args.push(remote.into());
        }
        Direction::FromServer => {
            args.push(format!("{remote}/").into());
            args.push(format!("{}/", spec.local.trim_end_matches('/')).into());
        }
    }
    args
}

/// User excludes, one pattern per line. Rule prefixes and merge directives are
/// refused, so an exclude can't turn into an include or read another file.
pub fn validate_excludes(lines: &[String]) -> AppResult<Vec<String>> {
    let mut out = Vec::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        // Short rules (`+ x`, `-,s x`, `. file`) and long ones (`merge file`); a
        // plain name like `merge.log` stays a pattern.
        let word = line.split([' ', ',']).next().unwrap_or("");
        let rule = line == "!"
            || (line.len() > word.len()
                && ["+", "-", ".", ":", "merge", "dir-merge", "include", "exclude", "protect", "hide", "show", "risk", "clear"]
                    .contains(&word));
        if rule || line.contains(['\n', '\0']) {
            return Err(AppError::other(tr!(
                "Exclude pattern not allowed: {line}",
                "Uitsluitpatroon niet toegestaan: {line}",
                line = line
            )));
        }
        // `#` and `;` start comments in an exclude file.
        if line.starts_with(['#', ';']) {
            continue;
        }
        out.push(line.to_string());
    }
    Ok(out)
}

/// `rel` relative to `root`, when `path` is `root` itself or inside it.
pub fn relative<'a>(root: &str, path: &'a str) -> Option<&'a str> {
    let root = root.trim_end_matches('/');
    let rest = path.trim_end_matches('/').strip_prefix(root)?;
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix('/')
    }
}

/// A pattern that matches exactly `rel` below the transfer root.
pub fn anchored(rel: &str, is_dir: bool) -> String {
    // Backslashes only escape when the pattern has wildcards; otherwise they are literal.
    let rel = if rel.contains(['*', '?', '[']) {
        rel.chars().fold(String::new(), |mut s, c| {
            if matches!(c, '*' | '?' | '[' | '\\') {
                s.push('\\');
            }
            s.push(c);
            s
        })
    } else {
        rel.to_string()
    };
    format!("/{rel}{}", if is_dir { "/" } else { "" })
}

/// Kade's backup folders never take part in a sync, on either side: the
/// source's isn't copied, and the destination's is protected from deletes.
pub fn backup_root_excludes(roots: &[(&str, Option<&str>)]) -> Vec<String> {
    roots
        .iter()
        .filter_map(|(root, backups)| relative(root, (*backups)?))
        .filter(|rel| !rel.is_empty())
        .map(|rel| anchored(rel, true))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(direction: Direction) -> SyncRequest {
        SyncRequest {
            direction,
            local: "/Users/me/site".into(),
            remote: "/var/www/site".into(),
            delete: true,
            checksum: false,
            excludes: vec![],
        }
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|a| a.into_string().unwrap()).collect()
    }

    #[test]
    fn builds_each_pass() {
        let rsh = Path::new("/tmp/kade-rsync-x/rsh");
        let ex = Path::new("/tmp/kade-rsync-x/excludes");
        let push = strings(build(&spec(Direction::ToServer), Pass::Preview, None, rsh, ex));
        assert!(push.ends_with(&["-e".into(), rsh.to_str().unwrap().into(), "/Users/me/site/".into(), "kade:KADE_REMOTE".into()]));
        assert!(push.contains(&"-n".into()) && push.contains(&"--delete-after".into()));
        assert!(push.contains(&"--exclude-from=/tmp/kade-rsync-x/excludes".into()));
        for never in ["-p", "-o", "-g", "-E", "--executability", "-s", "-z", "--partial", "--inplace", "--rsync-path", "-L", "-k"] {
            assert!(!push.iter().any(|a| a == never), "{never}");
        }

        let run = strings(build(&spec(Direction::FromServer), Pass::Run, Some("/data/kade/backups/t/Users/me/site"), rsh, ex));
        assert!(run.ends_with(&["kade:KADE_REMOTE/".into(), "/Users/me/site/".into()]));
        assert!(run.contains(&"--backup-dir=/data/kade/backups/t/Users/me/site".into()) && run.contains(&"-b".into()));
        assert!(!run.iter().any(|a| a.starts_with("--delete") || a == "-n"), "the real run never deletes");

        let del = strings(build(&spec(Direction::ToServer), Pass::Deletions, None, rsh, ex));
        assert!(del.contains(&"-n".into()) && del.contains(&"--delete-after".into()));
        let mut no_delete = spec(Direction::ToServer);
        no_delete.delete = false;
        no_delete.checksum = true;
        let pre = strings(build(&no_delete, Pass::Preview, None, rsh, ex));
        assert!(!pre.contains(&"--delete-after".into()) && pre.contains(&"-c".into()));
    }

    #[test]
    fn validates_excludes() {
        let ok = validate_excludes(&[".DS_Store".into(), "".into(), "node_modules/".into(), "# note".into(), "*.log\r".into()]).unwrap();
        assert_eq!(ok, [".DS_Store", "node_modules/", "*.log"]);
        assert_eq!(validate_excludes(&["merge.log".into(), "exclude-me".into(), "-x".into()]).unwrap().len(), 3);
        for bad in ["+ keep", "- x", "+,s x", "!", ". other-file", ": .rsync-filter", "merge /etc/x", "dir-merge .f", "include x"] {
            assert!(validate_excludes(&[bad.into()]).is_err(), "{bad}");
        }
    }

    #[test]
    fn excludes_backup_roots_inside_the_tree() {
        let ex = backup_root_excludes(&[
            ("/home/me", Some("/home/me/.cache/kade")),
            ("/var/www", Some("/home/me/.cache/kade")),
            ("/Users/me/Library", Some("/Users/me/Library/Application Support/kade/backups")),
            ("/x", None),
        ]);
        assert_eq!(ex, ["/.cache/kade/", "/Application Support/kade/backups/"]);
        assert_eq!(anchored("a[1]/b*", false), "/a\\[1]/b\\*");
        assert_eq!(anchored("back\\slash", false), "/back\\slash");
        assert_eq!(relative("/var/www/", "/var/www/site"), Some("site"));
        assert_eq!(relative("/var/www", "/var/wwwx"), None);
    }
}
