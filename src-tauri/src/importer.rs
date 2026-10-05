//! Import connections from Cyberduck, FileZilla and Transmit.
//!
//! Only what Kade can use comes over: protocol, host, port, user, name,
//! folder/group, remote and local path, and a key file. Passwords never do;
//! Kade asks for them at connect time like for any other connection.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{AppError, AppResult};
use crate::profiles::{Auth, Protocol, ServerProfile};
use crate::store;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Cyberduck,
    Filezilla,
    Transmit,
}

/// Bookmarks of another app found at its usual place on this computer.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub source: Source,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub profile: ServerProfile,
    /// Kade already has a connection with this protocol, host, port and user.
    pub duplicate: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub candidates: Vec<Candidate>,
    /// Bookmarks left out, with the reason (S3, WebDAV, no host, …).
    pub skipped: Vec<String>,
}

/// A bookmark as read from the other app, before it becomes a profile.
#[derive(Debug, Default, Clone, PartialEq)]
struct Entry {
    name: String,
    protocol: Option<Protocol>,
    host: String,
    port: Option<u16>,
    user: String,
    group: String,
    key_file: Option<String>,
    remote_path: Option<String>,
    local_path: Option<String>,
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

fn candidates(source: Source) -> Vec<PathBuf> {
    let h = home();
    match source {
        Source::Cyberduck => vec![
            h.join("Library/Group Containers/G69SCX94XU.duck/Library/Application Support/duck/Bookmarks"),
            h.join("Library/Application Support/Cyberduck/Bookmarks"),
            h.join(".duck/bookmarks"),
        ],
        Source::Filezilla => vec![h.join(".config/filezilla/sitemanager.xml"), h.join(".filezilla/sitemanager.xml")],
        // Transmit 5 keeps its servers in a private database; it has to export them.
        Source::Transmit => Vec::new(),
    }
}

/// Bookmark files of the three apps in their default locations.
pub fn detect() -> Vec<Found> {
    let mut out = Vec::new();
    for source in [Source::Cyberduck, Source::Filezilla, Source::Transmit] {
        for path in candidates(source) {
            let usable = if path.is_dir() { !duck_files(&path).is_empty() } else { path.is_file() };
            if usable {
                out.push(Found { source, path: path.to_string_lossy().into_owned() });
            }
        }
    }
    out
}

pub fn preview(source: Source, path: &str) -> AppResult<Preview> {
    let path = PathBuf::from(path);
    let mut skipped = Vec::new();
    let entries = match source {
        Source::Cyberduck => parse_cyberduck(&path, &mut skipped)?,
        Source::Filezilla => parse_filezilla(&std::fs::read_to_string(&path)?, &mut skipped)?,
        Source::Transmit => parse_transmit(&std::fs::read(&path)?, &mut skipped)?,
    };

    let mut seen: HashSet<String> = store::load()?.servers.iter().map(identity).collect();
    let candidates = entries
        .into_iter()
        .map(|e| {
            let profile = to_profile(e);
            let duplicate = !seen.insert(identity(&profile));
            Candidate { profile, duplicate }
        })
        .collect();
    Ok(Preview { candidates, skipped })
}

/// Save the chosen connections in `workspace`. Returns how many were added.
pub fn apply(profiles: Vec<ServerProfile>, workspace: &str) -> AppResult<usize> {
    let now = store::now_ms();
    store::update(|data| {
        let n = profiles.len();
        for mut p in profiles {
            p.id = uuid::Uuid::new_v4().to_string();
            p.workspace = workspace.to_string();
            p.tunnels.clear();
            p.updated_at = now;
            data.servers.push(p);
        }
        Ok(n)
    })
}

/// What makes two connections the same one. SSH and SFTP share a login.
fn identity(p: &ServerProfile) -> String {
    let kind = match p.protocol {
        Protocol::Sftp | Protocol::Ssh => "ssh",
        Protocol::Ftp | Protocol::Ftps => "ftp",
    };
    format!("{kind}|{}|{}|{}", p.host.to_lowercase(), p.port, p.user)
}

fn to_profile(e: Entry) -> ServerProfile {
    let protocol = e.protocol.unwrap_or(Protocol::Sftp);
    let port = e.port.filter(|&p| p != 0).unwrap_or(match protocol {
        Protocol::Sftp | Protocol::Ssh => 22,
        Protocol::Ftp | Protocol::Ftps => 21,
    });
    let auth = match (&protocol, e.key_file) {
        (Protocol::Sftp | Protocol::Ssh, Some(path)) => Auth::KeyFile { path },
        _ => Auth::Password,
    };
    let name = if e.name.trim().is_empty() { e.host.clone() } else { e.name.trim().to_string() };
    let user = if e.user.is_empty() && matches!(protocol, Protocol::Ftp | Protocol::Ftps) { "anonymous".into() } else { e.user };
    ServerProfile {
        id: String::new(),
        name,
        protocol,
        host: e.host.trim().to_string(),
        port,
        user,
        group: e.group.trim().to_string(),
        auth,
        remote_path: e.remote_path.filter(|p| !p.trim().is_empty()),
        local_path: e.local_path.filter(|p| !p.trim().is_empty()),
        workspace: String::new(),
        tunnels: Vec::new(),
        updated_at: 0,
    }
}

/// Keep an entry only when Kade can connect to it; otherwise say why not.
fn keep(e: Entry, protocol_name: &str, skipped: &mut Vec<String>, out: &mut Vec<Entry>) {
    let label = if e.name.is_empty() { e.host.clone() } else { e.name.clone() };
    if e.host.trim().is_empty() {
        skipped.push(tr!("{label}: no host", "{label}: geen host", label = label));
    } else if e.protocol.is_none() {
        skipped.push(tr!("{label}: {p} isn't supported", "{label}: {p} wordt niet ondersteund", label = label, p = protocol_name));
    } else {
        out.push(e);
    }
}

// ---------------------------------------------------------------- Cyberduck

fn duck_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> =
        read.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "duck")).collect();
    files.sort();
    files
}

/// A Bookmarks folder, or one `.duck` file (an XML plist per bookmark).
fn parse_cyberduck(path: &Path, skipped: &mut Vec<String>) -> AppResult<Vec<Entry>> {
    let files = if path.is_dir() { duck_files(path) } else { vec![path.to_path_buf()] };
    let mut out = Vec::new();
    for file in files {
        let value = plist::Value::from_file(&file).map_err(|e| unreadable(&file, e))?;
        let Some(dict) = value.as_dictionary() else { continue };
        let s = |k: &str| dict.get(k).and_then(|v| v.as_string()).unwrap_or_default().to_string();
        let nested = |k: &str| dict.get(k).and_then(|v| v.as_dictionary()).and_then(|d| d.get("Path")).and_then(|v| v.as_string());
        let protocol_name = s("Protocol");
        let entry = Entry {
            name: s("Nickname"),
            protocol: match protocol_name.as_str() {
                "sftp" => Some(Protocol::Sftp),
                "ftp" => Some(Protocol::Ftp),
                "ftps" => Some(Protocol::Ftps),
                _ => None,
            },
            host: s("Hostname"),
            port: dict.get("Port").and_then(plist_u16),
            user: s("Username"),
            group: dict
                .get("Labels")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_string())
                .unwrap_or_default()
                .to_string(),
            key_file: nested("Private Key File Dictionary")
                .map(str::to_string)
                .or_else(|| dict.get("Private Key File").and_then(|v| v.as_string()).map(str::to_string))
                .filter(|p| !p.is_empty()),
            remote_path: Some(s("Path")),
            local_path: nested("Local Folder Dictionary")
                .map(str::to_string)
                .or_else(|| dict.get("Local Folder").and_then(|v| v.as_string()).map(str::to_string)),
        };
        keep(entry, &protocol_name, skipped, &mut out);
    }
    Ok(out)
}

fn plist_u16(v: &plist::Value) -> Option<u16> {
    match v {
        plist::Value::Integer(i) => i.as_unsigned().and_then(|n| u16::try_from(n).ok()),
        plist::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------- FileZilla

/// `sitemanager.xml`, or a Site Manager export (same format).
fn parse_filezilla(xml: &str, skipped: &mut Vec<String>) -> AppResult<Vec<Entry>> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| AppError::other(tr!("Not a FileZilla file: {e}", "Geen FileZilla-bestand: {e}", e = e)))?;
    let servers = doc
        .descendants()
        .find(|n| n.has_tag_name("Servers"))
        .ok_or_else(|| AppError::other(tr!("No Site Manager entries in this file", "Geen Sitebeheer-items in dit bestand")))?;
    let mut out = Vec::new();
    walk_filezilla(servers, &[], skipped, &mut out);
    Ok(out)
}

fn walk_filezilla(node: roxmltree::Node, folders: &[String], skipped: &mut Vec<String>, out: &mut Vec<Entry>) {
    for child in node.children().filter(|n| n.is_element()) {
        if child.has_tag_name("Folder") {
            // The folder's name is its own text, before the nested elements.
            let name = child.children().find(|n| n.is_text()).and_then(|n| n.text()).unwrap_or_default().trim().to_string();
            let mut path = folders.to_vec();
            path.push(name);
            walk_filezilla(child, &path, skipped, out);
        } else if child.has_tag_name("Server") {
            let field =
                |tag: &str| child.children().find(|n| n.has_tag_name(tag)).and_then(|n| n.text()).unwrap_or_default().trim().to_string();
            let code = field("Protocol");
            let protocol = match code.as_str() {
                "0" | "6" | "" => Some(Protocol::Ftp),
                "1" => Some(Protocol::Sftp),
                "3" | "4" => Some(Protocol::Ftps),
                _ => None,
            };
            let mut port = field("Port").parse().ok();
            if code == "3" && port.is_none_or(|p| p == 0) {
                port = Some(990);
            }
            let anonymous = field("Logontype") == "0";
            let key = field("Keyfile");
            let entry = Entry {
                name: field("Name"),
                protocol,
                host: field("Host"),
                port,
                user: if anonymous { "anonymous".into() } else { field("User") },
                group: folders.iter().filter(|f| !f.is_empty()).cloned().collect::<Vec<_>>().join(" / "),
                key_file: (field("Logontype") == "5" && !key.is_empty()).then_some(key),
                remote_path: decode_remote_dir(&field("RemoteDir")),
                local_path: Some(field("LocalDir")),
            };
            let protocol_name = match code.as_str() {
                "2" => "Telnet",
                "5" => "S3",
                _ => "this protocol",
            };
            keep(entry, protocol_name, skipped, out);
        }
    }
}

/// FileZilla stores a remote directory as `type prefixlen [prefix] (len segment)*`,
/// e.g. `1 0 4 home 8 my files` → `/home/my files`. Lengths count characters,
/// so segments may contain spaces.
fn decode_remote_dir(raw: &str) -> Option<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    let number = |i: &mut usize| -> Option<usize> {
        while chars.get(*i) == Some(&' ') {
            *i += 1;
        }
        let start = *i;
        while chars.get(*i).is_some_and(|c| c.is_ascii_digit()) {
            *i += 1;
        }
        chars[start..*i].iter().collect::<String>().parse().ok()
    };
    number(&mut i)?; // server type
    let prefix_len = number(&mut i)?;
    let take = |i: &mut usize, n: usize| -> Option<String> {
        *i += 1; // the space before the text
        let end = *i + n;
        let s = chars.get(*i..end)?.iter().collect();
        *i = end;
        Some(s)
    };
    if prefix_len > 0 {
        take(&mut i, prefix_len)?;
    }
    let mut segments = Vec::new();
    while i < chars.len() {
        let Some(n) = number(&mut i) else { break };
        segments.push(take(&mut i, n)?);
    }
    if segments.is_empty() {
        return None;
    }
    Some(format!("/{}", segments.join("/")))
}

// ---------------------------------------------------------------- Transmit

/// A Transmit export (Servers → Export…, without passwords), or Transmit 4's
/// favorites. These are property lists, often an NSKeyedArchiver archive,
/// sometimes JSON; rather than depend on one layout we look for anything
/// shaped like a server and take the name of the collection it sits in as
/// its group.
fn parse_transmit(bytes: &[u8], skipped: &mut Vec<String>) -> AppResult<Vec<Entry>> {
    let trimmed = bytes.trim_ascii_start();
    let tree = if trimmed.starts_with(b"{") || trimmed.starts_with(b"[") {
        serde_json::from_slice::<Value>(trimmed).ok()
    } else {
        plist::Value::from_reader(std::io::Cursor::new(bytes)).ok().map(plist_to_json)
    };
    let Some(tree) = tree else {
        return Err(AppError::other(tr!(
            "Kade can't read this file. Transmit encrypts exports that include passwords: export again with “Include passwords” off.",
            "Kade kan dit bestand niet lezen. Transmit versleutelt exports met wachtwoorden: exporteer opnieuw met “Include passwords” uit."
        )));
    };
    let tree = unarchive(tree);
    let mut out = Vec::new();
    walk_transmit(&tree, "", skipped, &mut out);
    if out.is_empty() && skipped.is_empty() {
        return Err(AppError::other(tr!("No servers found in this file", "Geen servers gevonden in dit bestand")));
    }
    Ok(out)
}

fn plist_to_json(v: plist::Value) -> Value {
    match v {
        plist::Value::Dictionary(d) => Value::Object(d.into_iter().map(|(k, v)| (k, plist_to_json(v))).collect()),
        plist::Value::Array(a) => Value::Array(a.into_iter().map(plist_to_json).collect()),
        plist::Value::String(s) => Value::String(s),
        plist::Value::Boolean(b) => Value::Bool(b),
        plist::Value::Integer(i) => i.as_signed().map(Value::from).or_else(|| i.as_unsigned().map(Value::from)).unwrap_or(Value::Null),
        plist::Value::Real(r) => Value::from(r),
        plist::Value::Uid(u) => serde_json::json!({ "$uid": u.get() }),
        // Archives nest archives (Transmit 4 keeps its collections in a data blob).
        plist::Value::Data(d) => plist::Value::from_reader(std::io::Cursor::new(d)).map(plist_to_json).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// Turn an NSKeyedArchiver archive (`$objects` + `$top`) into a plain tree,
/// anywhere in `v`. Other values pass through unchanged.
fn unarchive(v: Value) -> Value {
    match v {
        Value::Object(map) if map.contains_key("$objects") && map.contains_key("$top") => {
            let objects = map.get("$objects").and_then(|o| o.as_array()).cloned().unwrap_or_default();
            let objects: Vec<Value> = objects.into_iter().map(unarchive).collect();
            let top = map.get("$top").cloned().unwrap_or(Value::Null);
            resolve(&top, &objects, 0)
        }
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, unarchive(v))).collect()),
        Value::Array(a) => Value::Array(a.into_iter().map(unarchive).collect()),
        other => other,
    }
}

fn resolve(v: &Value, objects: &[Value], depth: usize) -> Value {
    if depth > 64 {
        return Value::Null;
    }
    let next = |v: &Value| resolve(v, objects, depth + 1);
    match v {
        Value::Object(map) => {
            if let Some(uid) = map.get("$uid").and_then(|u| u.as_u64()) {
                return match objects.get(uid as usize) {
                    Some(Value::String(s)) if s == "$null" => Value::Null,
                    Some(o) => next(o),
                    None => Value::Null,
                };
            }
            if let Some(s) = map.get("NS.string") {
                return next(s);
            }
            if let (Some(Value::Array(keys)), Some(Value::Array(vals))) = (map.get("NS.keys"), map.get("NS.objects")) {
                let mut out = Map::new();
                for (k, v) in keys.iter().zip(vals) {
                    if let Value::String(k) = next(k) {
                        out.insert(k, next(v));
                    }
                }
                return Value::Object(out);
            }
            if let Some(Value::Array(items)) = map.get("NS.objects") {
                return Value::Array(items.iter().map(next).collect());
            }
            Value::Object(map.iter().filter(|(k, _)| k.as_str() != "$class").map(|(k, v)| (k.clone(), next(v))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(next).collect()),
        other => other.clone(),
    }
}

/// Case-insensitive lookup of the first key that is present.
fn get<'a>(map: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| map.iter().find(|(name, v)| name.eq_ignore_ascii_case(k) && !v.is_null()).map(|(_, v)| v))
}

fn get_str(map: &Map<String, Value>, keys: &[&str]) -> String {
    get(map, keys).and_then(|v| v.as_str()).unwrap_or_default().to_string()
}

fn walk_transmit(v: &Value, group: &str, skipped: &mut Vec<String>, out: &mut Vec<Entry>) {
    match v {
        Value::Object(map) => {
            if get(map, &["address", "server", "hostname", "host"]).is_some_and(|h| h.is_string()) {
                let (protocol, protocol_name) = transmit_protocol(get(map, &["protocol"]));
                let entry = Entry {
                    name: get_str(map, &["name", "nickname"]),
                    protocol,
                    host: get_str(map, &["address", "server", "hostname", "host"]),
                    port: get(map, &["port"])
                        .and_then(|p| p.as_u64().or_else(|| p.as_str().and_then(|s| s.trim().parse().ok())))
                        .and_then(|p| u16::try_from(p).ok()),
                    user: get_str(map, &["username", "user", "login"]),
                    group: group.to_string(),
                    key_file: Some(get_str(map, &["keyFile", "identityFile", "privateKeyPath", "sshKeyPath"])).filter(|k| !k.is_empty()),
                    remote_path: Some(get_str(map, &["remotePath", "initialRemotePath", "path"])),
                    local_path: Some(get_str(map, &["localPath", "initialLocalPath"])),
                };
                keep(entry, &protocol_name, skipped, out);
                return;
            }
            let name = get_str(map, &["name"]);
            if name == "History" {
                return;
            }
            let is_collection = !name.is_empty() && map.values().any(|v| v.is_array());
            let group = if is_collection { name.as_str() } else { group };
            for child in map.values() {
                walk_transmit(child, group, skipped, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| walk_transmit(i, group, skipped, out)),
        _ => {}
    }
}

/// Transmit writes protocols as names (Transmit 4) or as four-character codes
/// stored as a number (Transmit 5: 1397118032 is `SFTP`).
fn transmit_protocol(v: Option<&Value>) -> (Option<Protocol>, String) {
    let name = match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(|n| String::from_utf8_lossy(&n.to_be_bytes()).trim().to_string())
            .unwrap_or_default(),
        // No protocol at all: most likely SFTP.
        _ => return (Some(Protocol::Sftp), "SFTP".into()),
    };
    let upper = name.to_uppercase();
    let protocol = match upper.as_str() {
        "SFTP" => Some(Protocol::Sftp),
        "FTP" => Some(Protocol::Ftp),
        p if p.starts_with("FTP") => Some(Protocol::Ftps),
        _ => None,
    };
    (protocol, name)
}

fn unreadable(path: &Path, e: impl std::fmt::Display) -> AppError {
    AppError::other(tr!("Can't read {path}: {e}", "Kan {path} niet lezen: {e}", path = path.display(), e = e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_dir() {
        assert_eq!(decode_remote_dir("1 0 4 home 4 jurn").as_deref(), Some("/home/jurn"));
        assert_eq!(decode_remote_dir("1 0 3 var 8 my files").as_deref(), Some("/var/my files"));
        assert_eq!(decode_remote_dir("1 0").as_deref(), None);
        assert_eq!(decode_remote_dir("").as_deref(), None);
    }

    #[test]
    fn filezilla_sites_and_folders() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FileZilla3 version="3.66.0" platform="unix">
  <Servers>
    <Server>
      <Host>ftp.example.com</Host><Port>21</Port><Protocol>4</Protocol><Type>0</Type>
      <User>web</User><Logontype>1</Logontype><Name>Shop</Name>
      <RemoteDir>1 0 6 public</RemoteDir><LocalDir>/home/me/shop</LocalDir>
    </Server>
    <Folder expanded="1">Clients
      <Folder>Acme
        <Server>
          <Host>ssh.example.com</Host><Port>2222</Port><Protocol>1</Protocol>
          <User>deploy</User><Logontype>5</Logontype><Keyfile>/home/me/.ssh/id_ed25519</Keyfile>
          <Name>Acme prod</Name>
        </Server>
      </Folder>
      <Server><Host>s3.example.com</Host><Protocol>5</Protocol><Name>Bucket</Name></Server>
    </Folder>
  </Servers>
</FileZilla3>"#;
        let mut skipped = Vec::new();
        let e = parse_filezilla(xml, &mut skipped).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].protocol, e[0].remote_path.as_deref()), (Some(Protocol::Ftps), Some("/public")));
        assert_eq!(e[1].group, "Clients / Acme");
        assert_eq!(e[1].port, Some(2222));
        assert_eq!(e[1].key_file.as_deref(), Some("/home/me/.ssh/id_ed25519"));
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn cyberduck_bookmark() {
        let dir = std::env::temp_dir().join(format!("kade-duck-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.duck"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Protocol</key><string>sftp</string>
  <key>Nickname</key><string>Web</string>
  <key>Hostname</key><string>web.example.com</string>
  <key>Port</key><string>22</string>
  <key>Username</key><string>deploy</string>
  <key>Path</key><string>/var/www</string>
  <key>Labels</key><array><string>Work</string></array>
  <key>Private Key File Dictionary</key><dict><key>Path</key><string>~/.ssh/id_ed25519</string></dict>
</dict></plist>"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("b.duck"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>Protocol</key><string>s3</string><key>Hostname</key><string>s3.amazonaws.com</string></dict></plist>"#,
        )
        .unwrap();
        let mut skipped = Vec::new();
        let e = parse_cyberduck(&dir, &mut skipped).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].name.as_str(), e[0].group.as_str(), e[0].port), ("Web", "Work", Some(22)));
        assert_eq!(e[0].key_file.as_deref(), Some("~/.ssh/id_ed25519"));
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn transmit_json_export() {
        let json = r#"{"__NSArrayM":{"NS.object.0":{"name":"Hosting","connections":[
            {"protocol":1397118032,"address":"10.0.0.5","port":2200,"username":"sftpuser","remotePath":"/home/sftpuser/apps","name":"App"},
            {"protocol":"WebDAV","address":"dav.example.com","name":"Dav"}]}}}"#;
        let mut skipped = Vec::new();
        let e = parse_transmit(json.as_bytes(), &mut skipped).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].protocol, e[0].group.as_str(), e[0].port), (Some(Protocol::Sftp), "Hosting", Some(2200)));
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn transmit_keyed_archive() {
        // A tiny NSKeyedArchiver archive: one collection holding one favorite.
        let mut fav = plist::Dictionary::new();
        fav.insert("server".into(), plist::Value::Uid(plist::Uid::new(4)));
        fav.insert("protocol".into(), "FTPTLS".into());
        fav.insert("nickname".into(), "Old site".into());
        let mut coll = plist::Dictionary::new();
        coll.insert("name".into(), "Clients".into());
        coll.insert("favorites".into(), plist::Value::Uid(plist::Uid::new(3)));
        let mut arr = plist::Dictionary::new();
        arr.insert("NS.objects".into(), plist::Value::Array(vec![plist::Value::Uid(plist::Uid::new(2))]));
        let objects = vec![
            "$null".into(),
            plist::Value::Dictionary(coll.clone()),
            plist::Value::Dictionary(fav),
            plist::Value::Dictionary(arr),
            "ftp.example.com".into(),
        ];
        let mut top = plist::Dictionary::new();
        top.insert("root".into(), plist::Value::Uid(plist::Uid::new(1)));
        let mut archive = plist::Dictionary::new();
        archive.insert("$archiver".into(), "NSKeyedArchiver".into());
        archive.insert("$top".into(), plist::Value::Dictionary(top));
        archive.insert("$objects".into(), plist::Value::Array(objects));
        let mut bytes = Vec::new();
        plist::Value::Dictionary(archive).to_writer_binary(&mut bytes).unwrap();

        let mut skipped = Vec::new();
        let e = parse_transmit(&bytes, &mut skipped).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].host.as_str(), e[0].group.as_str(), e[0].protocol), ("ftp.example.com", "Clients", Some(Protocol::Ftps)));
    }

    #[test]
    fn encrypted_transmit_file_is_explained() {
        assert!(parse_transmit(&[0x8f, 0x12, 0x00, 0x99, 0x42], &mut Vec::new()).is_err());
    }
}
