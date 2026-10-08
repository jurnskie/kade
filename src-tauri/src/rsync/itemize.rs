//! Parse rsync's `--out-format='%i %l %n'` lines.
//!
//! The change code is 9 columns (protocol 29: openrsync, 2.6.9) or 11 (GNU
//! 3.x), e.g. `>f.st......`. Names have control characters escaped as
//! `\#ooo` (`-8` keeps UTF-8 as is); directories end in `/`.
//!
//! Deletions don't follow the format everywhere: openrsync prints
//! `*deleting name`, GNU pads the code and then prints the length,
//! `*deleting   0 name`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A file is sent: new (`+++++++`) or replacing an existing one.
    File {
        new: bool,
    },
    /// A symlink, device or special file is created or changed.
    Other {
        new: bool,
    },
    Dir {
        new: bool,
    },
    Deleted,
    /// Only attributes change (`.f..t....`): nothing is replaced.
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    pub size: u64,
    /// Relative to the transfer root, unescaped. A trailing `/` (directory) is removed.
    pub name: String,
    pub is_dir: bool,
}

/// `\#012` → newline. Other backslashes are literal (rsync doesn't escape them).
pub fn unescape(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'\\' && raw.get(i + 1) == Some(&b'#') {
            if let Some(oct) = raw.get(i + 2..i + 5) {
                if oct.iter().all(|c| (b'0'..=b'7').contains(c)) {
                    let value = oct.iter().fold(0u32, |acc, c| acc * 8 + u32::from(c - b'0'));
                    if let Ok(byte) = u8::try_from(value) {
                        out.push(byte);
                        i += 5;
                        continue;
                    }
                }
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

fn name(raw: &[u8]) -> Option<(String, bool)> {
    let bytes = unescape(raw);
    let name = String::from_utf8_lossy(&bytes).into_owned();
    let is_dir = name.ends_with('/');
    let name = name.trim_end_matches('/').to_string();
    // The root itself (`./`) is not an item.
    (!name.is_empty() && name != ".").then_some((name, is_dir))
}

fn split_once(line: &[u8], at: u8) -> Option<(&[u8], &[u8])> {
    let i = line.iter().position(|&c| c == at)?;
    Some((&line[..i], &line[i + 1..]))
}

/// One line of stdout; `None` for anything that isn't an item (warnings, blank lines).
pub fn parse_line(line: &[u8]) -> Option<Item> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if let Some(rest) = line.strip_prefix(b"*deleting") {
        // openrsync: one space and the name. GNU: padding, a length, the name.
        let rest = rest.strip_prefix(b" ")?;
        let rest = if rest.first() == Some(&b' ') {
            let rest = &rest[rest.iter().position(|&c| c != b' ')?..];
            match split_once(rest, b' ') {
                Some((n, name)) if !n.is_empty() && n.iter().all(u8::is_ascii_digit) => name,
                _ => rest,
            }
        } else {
            rest
        };
        let (name, is_dir) = name(rest)?;
        return Some(Item { kind: Kind::Deleted, size: 0, name, is_dir });
    }
    let (code, rest) = split_once(line, b' ')?;
    let (size, rest) = split_once(rest, b' ')?;
    if !(code.len() == 9 || code.len() == 11) || !matches!(code[0], b'<' | b'>' | b'c' | b'h' | b'.') {
        return None;
    }
    let size = std::str::from_utf8(size).ok()?.parse().ok()?;
    let (name, is_dir) = name(rest)?;
    let new = code[2..].iter().all(|&c| c == b'+');
    let kind = match (code[0], code[1]) {
        (_, b'd') => Kind::Dir { new },
        (b'.', _) => Kind::Unchanged,
        (b'<' | b'>', b'f') => Kind::File { new },
        _ => Kind::Other { new },
    };
    Some(Item { kind, size, name, is_dir })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(line: &str) -> Item {
        parse_line(line.as_bytes()).unwrap_or_else(|| panic!("{line:?} didn't parse"))
    }

    #[test]
    fn parses_change_codes() {
        // 9 columns (protocol 29) and 11 (GNU 3.x).
        assert_eq!(item(">f+++++++ 12 a.txt").kind, Kind::File { new: true });
        assert_eq!(item(">f+++++++++ 12 a.txt").kind, Kind::File { new: true });
        assert_eq!(item(">f.st.... 8 edit.txt"), Item { kind: Kind::File { new: false }, size: 8, name: "edit.txt".into(), is_dir: false });
        assert_eq!(item(">f..T...... 3 b").kind, Kind::File { new: false });
        assert_eq!(item("<f.s..... 8 sub/x").kind, Kind::File { new: false });
        assert_eq!(item("<f+++++++ 0 x").kind, Kind::File { new: true });
        let d = item("cd+++++++ 96 sub/");
        assert_eq!((d.kind, d.name.as_str(), d.is_dir), (Kind::Dir { new: true }, "sub", true));
        assert_eq!(item(".d..t.... 96 sub/").kind, Kind::Dir { new: false });
        assert_eq!(item(".f..t...... 4 same").kind, Kind::Unchanged);
        assert_eq!(item("cL+++++++ 3 ln").kind, Kind::Other { new: true });
        // `%n` never adds a link target, so an arrow is part of the name.
        assert_eq!(item("cL+++++++ 3 a -> b").name, "a -> b");
        assert!(parse_line(b"cd+++++++ 192 ./").is_none(), "the root is not an item");
    }

    #[test]
    fn parses_deletions_of_both_implementations() {
        let open = item("*deleting olddir/");
        assert_eq!((open.kind, open.name.as_str(), open.is_dir), (Kind::Deleted, "olddir", true));
        assert_eq!(item("*deleting olddir/z").name, "olddir/z");
        assert_eq!(item("*deleting   0 dir/").name, "dir");
        assert_eq!(item("*deleting   0 a b").name, "a b");
        // Captured from GNU rsync 3.5.1 (`-i --out-format='%i %l %n'`): the length is
        // always there, so a name that starts with digits stays whole.
        assert_eq!(item("*deleting   0 gone/y").name, "gone/y");
        assert_eq!(item("*deleting   0 7 digits name").name, "7 digits name");
        assert_eq!(item("*deleting   0 gone/").name, "gone");
        // And its 11-column codes.
        assert_eq!(item(">f.st...... 2 a").kind, Kind::File { new: false });
        assert_eq!(item("cL+++++++++ 3 lnk").kind, Kind::Other { new: true });
        assert_eq!(item(">f+++++++++ 4 sub/n").kind, Kind::File { new: true });
    }

    #[test]
    fn names_keep_spaces_and_decode_escapes() {
        assert_eq!(item(">f+++++++ 0 sub/a b.txt").name, "sub/a b.txt");
        assert_eq!(item(">f+++++++ 0 new\\#012line").name, "new\nline");
        assert_eq!(item(">f+++++++ 0 back\\slash").name, "back\\slash");
        assert_eq!(item(">f+++++++ 0 tab\tx").name, "tab\tx");
        assert_eq!(item(">f+++++++ 0 $(touch pwned)").name, "$(touch pwned)");
        assert_eq!(item(">f+++++++ 0 üñí").name, "üñí");
        assert_eq!(unescape(b"a\\#00"), b"a\\#00", "a cut-off escape stays literal");
        assert_eq!(unescape(b"\\#999"), b"\\#999");
    }

    #[test]
    fn ignores_other_output() {
        for line in ["", "sending incremental file list", "Deletions stopped due to --max-delete limit (4 skipped)", "total size is 12"] {
            assert!(parse_line(line.as_bytes()).is_none(), "{line:?}");
        }
    }
}
