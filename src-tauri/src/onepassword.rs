//! Secrets from 1Password via its CLI (`op`), with the desktop-app integration
//! doing the unlocking. Kade stores only references (`op://vault/item/…`) plus
//! the account they belong to: on a machine signed in to several accounts
//! (work and personal), `op` would otherwise pick its default account.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Deserialize)]
struct RawAccount {
    account_uuid: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
struct RawVault {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct RawItem {
    id: String,
    title: String,
    vault: RawVault,
    #[serde(default)]
    additional_information: Option<String>,
    #[serde(default)]
    urls: Vec<RawUrl>,
}

#[derive(Deserialize)]
struct RawUrl {
    href: String,
}

#[derive(Serialize)]
pub struct OpAccount {
    /// Pass this as `account` on connections and in other calls.
    pub id: String,
    pub email: String,
    pub url: String,
}

#[derive(Serialize)]
pub struct OpVault {
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct OpItem {
    pub title: String,
    pub vault: String,
    pub vault_id: String,
    /// For logins, 1Password reports the username here.
    pub username: Option<String>,
    pub url: Option<String>,
    /// `op://<vault>/<item>/password`
    pub reference: String,
}

#[derive(Serialize, Clone)]
pub struct OpSshKey {
    pub title: String,
    pub vault: String,
    pub vault_id: String,
    /// `SHA256:…`, matching what the SSH agent reports.
    pub fingerprint: String,
    /// `op://<vault>/<item>`: where Kade fetches the key when the agent lacks it.
    pub item: String,
}

/// Long enough for a human to find the 1Password prompt and approve it.
const OP_TIMEOUT: Duration = Duration::from_secs(90);

async fn op_raw(args: &[&str], account: Option<&str>) -> AppResult<String> {
    let mut cmd = crate::hostenv::async_command("op");
    cmd.args(args);
    if let Some(account) = account.filter(|a| !a.is_empty()) {
        cmd.args(["--account", account]);
    }
    // An unanswered approval would otherwise hang forever; dropping the future on timeout kills `op`.
    cmd.kill_on_drop(true);
    let out = match tokio::time::timeout(OP_TIMEOUT, cmd.output()).await {
        Ok(out) => out,
        Err(_) => return Err(AppError::OnePassword { message: friendly(OpFailure::TimedOut) }),
    }
    .map_err(|e| AppError::AgentUnavailable {
        message: tr!("1Password CLI (op) not found: {e}", "1Password CLI (op) niet gevonden: {e}", e = e),
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::OnePassword { message: friendly(classify(&err)) });
    }
    String::from_utf8(out.stdout).map_err(AppError::other)
}

#[derive(Debug, PartialEq)]
enum OpFailure {
    TimedOut,
    Dismissed,
    NotSignedIn,
    AccountNotFound,
    Other(String),
}

/// `op` prefixes every error line with `[ERROR] 2026/10/08 12:00:00 `.
fn strip_prefix(line: &str) -> &str {
    let Some(rest) = line.trim().strip_prefix("[ERROR]") else { return line.trim() };
    let mut rest = rest.trim_start();
    for _ in 0..2 {
        match rest.split_once(' ') {
            Some((tok, tail)) if !tok.is_empty() && tok.chars().all(|c| c.is_ascii_digit() || c == '/' || c == ':') => {
                rest = tail.trim_start()
            }
            _ => break,
        }
    }
    rest
}

fn classify(stderr: &str) -> OpFailure {
    let text = stderr.lines().map(strip_prefix).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    let lower = text.to_lowercase();
    if lower.contains("prompt dismissed") || lower.contains("authorization denied") || lower.contains("authorization timeout") {
        OpFailure::Dismissed
    } else if lower.contains("not signed in")
        || lower.contains("not currently signed in")
        || lower.contains("no accounts configured")
        || lower.contains("sign in again")
    {
        OpFailure::NotSignedIn
    } else if lower.contains("account not found")
        || lower.contains("does not match a configured account")
        || lower.contains("isn't a signed-in account")
        || lower.contains("no account found")
    {
        OpFailure::AccountNotFound
    } else {
        OpFailure::Other(text)
    }
}

fn friendly(failure: OpFailure) -> String {
    match failure {
        OpFailure::TimedOut => tr!(
            "1Password didn't answer in time — approve the prompt and try again.",
            "1Password reageerde niet op tijd — keur de melding goed en probeer het opnieuw."
        ),
        OpFailure::Dismissed => tr!(
            "The 1Password prompt was dismissed — approve it to continue and try again.",
            "De 1Password-melding is weggedrukt — keur hem goed om door te gaan en probeer het opnieuw."
        ),
        OpFailure::NotSignedIn => tr!(
            "You're not signed in to 1Password. Unlock the 1Password app (with CLI integration on) and try again.",
            "Je bent niet ingelogd bij 1Password. Ontgrendel de 1Password-app (met CLI-integratie aan) en probeer het opnieuw."
        ),
        OpFailure::AccountNotFound => tr!(
            "That 1Password account wasn't found. Check the account in Settings → 1Password.",
            "Dat 1Password-account is niet gevonden. Controleer het account in Instellingen → 1Password."
        ),
        OpFailure::Other(m) if m.is_empty() => tr!("1Password failed", "1Password mislukte"),
        OpFailure::Other(m) => tr!("1Password: {m}", "1Password: {m}", m = m),
    }
}

pub async fn accounts() -> AppResult<Vec<OpAccount>> {
    let raw: Vec<RawAccount> = serde_json::from_str(&op_raw(&["account", "list", "--format", "json"], None).await?)?;
    Ok(raw.into_iter().map(|a| OpAccount { id: a.account_uuid, email: a.email, url: a.url }).collect())
}

pub async fn vaults(account: Option<&str>) -> AppResult<Vec<OpVault>> {
    let raw: Vec<RawVault> = serde_json::from_str(&op_raw(&["vault", "list", "--format", "json"], account).await?)?;
    let mut out: Vec<OpVault> = raw.into_iter().map(|v| OpVault { id: v.id, name: v.name }).collect();
    out.sort_by_cached_key(|v| v.name.to_lowercase());
    Ok(out)
}

/// Logins and passwords, optionally from one vault only.
pub async fn items(account: Option<&str>, vault: Option<&str>) -> AppResult<Vec<OpItem>> {
    let mut args = vec!["item", "list", "--categories", "Login,Password,Server", "--format", "json"];
    if let Some(v) = vault.filter(|v| !v.is_empty()) {
        args.extend(["--vault", v]);
    }
    let items: Vec<RawItem> = serde_json::from_str(&op_raw(&args, account).await?)?;
    let mut out: Vec<OpItem> = items
        .into_iter()
        .map(|i| OpItem {
            reference: format!("op://{}/{}/password", i.vault.id, i.id),
            title: i.title,
            vault: i.vault.name,
            vault_id: i.vault.id,
            username: i.additional_information.filter(|s| !s.is_empty()),
            url: i.urls.into_iter().next().map(|u| u.href),
        })
        .collect();
    out.sort_by_cached_key(|i| i.title.to_lowercase());
    Ok(out)
}

const SSH_KEYS_TTL: Duration = Duration::from_secs(600);

type SshKeysCache = HashMap<(String, String), (Instant, Vec<OpSshKey>)>;

fn ssh_keys_cache() -> &'static Mutex<SshKeysCache> {
    static CACHE: OnceLock<Mutex<SshKeysCache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// SSH key items with their fingerprints (no private keys are read). Results are
/// cached per (account, vault) for a few minutes, as every connect asks for them.
pub async fn ssh_keys(account: Option<&str>, vault: Option<&str>) -> AppResult<Vec<OpSshKey>> {
    let key = (account.unwrap_or_default().to_string(), vault.unwrap_or_default().to_string());
    if let Some((at, keys)) = ssh_keys_cache().lock().unwrap().get(&key) {
        if at.elapsed() < SSH_KEYS_TTL {
            return Ok(keys.clone());
        }
    }
    let keys = fetch_ssh_keys(account, vault).await?;
    ssh_keys_cache().lock().unwrap().insert(key, (Instant::now(), keys.clone()));
    Ok(keys)
}

async fn fetch_ssh_keys(account: Option<&str>, vault: Option<&str>) -> AppResult<Vec<OpSshKey>> {
    let mut args = vec!["item", "list", "--categories", "SSH Key", "--format", "json"];
    if let Some(v) = vault.filter(|v| !v.is_empty()) {
        args.extend(["--vault", v]);
    }
    let listed: Vec<RawItem> = serde_json::from_str(&op_raw(&args, account).await?)?;
    // One `op` call per key. The first may have to unlock 1Password, so it
    // runs alone; the rest then run a few at a time.
    let mut out = Vec::with_capacity(listed.len());
    let mut items = listed.into_iter();
    if let Some(first) = items.next() {
        out.push(ssh_key(first, account).await);
    }
    out.extend(stream::iter(items).map(|item| ssh_key(item, account)).buffer_unordered(4).collect::<Vec<_>>().await);
    out.sort_by_cached_key(|k| k.title.to_lowercase());
    Ok(out)
}

async fn ssh_key(item: RawItem, account: Option<&str>) -> OpSshKey {
    // Only the public fingerprint field; the private key stays in 1Password.
    let fp = op_raw(&["item", "get", &item.id, "--vault", &item.vault.id, "--fields", "fingerprint"], account).await.unwrap_or_default();
    OpSshKey {
        item: format!("op://{}/{}", item.vault.id, item.id),
        title: item.title,
        vault: item.vault.name,
        vault_id: item.vault.id,
        fingerprint: fp.trim().to_string(),
    }
}

fn check_reference(reference: &str) -> AppResult<()> {
    if reference.starts_with("op://") {
        Ok(())
    } else {
        Err(AppError::other(tr!("Invalid 1Password reference", "Ongeldige 1Password-verwijzing")))
    }
}

/// Read a secret; 1Password asks the user to approve.
pub async fn read(reference: &str, account: Option<&str>) -> AppResult<String> {
    check_reference(reference)?;
    op_raw(&["read", "--no-newline", reference], account).await
}

/// The private key of an SSH Key item, in OpenSSH format, for when the
/// 1Password SSH agent on this machine doesn't offer it.
pub async fn read_ssh_key(item: &str, account: Option<&str>) -> AppResult<String> {
    check_reference(item)?;
    // Field id, not label: labels are localized ("privésleutel").
    let reference = format!("{}/private_key?ssh-format=openssh", item.trim_end_matches('/'));
    op_raw(&["read", &reference], account).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_op_error_prefix() {
        assert_eq!(strip_prefix("[ERROR] 2026/10/08 12:00:00 authorization prompt dismissed"), "authorization prompt dismissed");
        assert_eq!(strip_prefix("plain message"), "plain message");
        assert_eq!(strip_prefix("[ERROR] 2026/10/08 12:00:00 2026 is a year"), "2026 is a year");
    }

    #[test]
    fn classifies_op_errors() {
        let c = |s| classify(s);
        assert_eq!(c("[ERROR] 2026/10/08 12:00:00 authorization prompt dismissed"), OpFailure::Dismissed);
        assert_eq!(c("[ERROR] 2026/10/08 12:00:00 You are not currently signed in. Please run `op signin`"), OpFailure::NotSignedIn);
        assert_eq!(c("[ERROR] 2026/10/08 12:00:00 account is not signed in"), OpFailure::NotSignedIn);
        assert_eq!(c("[ERROR] 2026/10/08 12:00:00 \"x\" does not match a configured account"), OpFailure::AccountNotFound);
        assert_eq!(c("[ERROR] 2026/10/08 12:00:00 something odd\n"), OpFailure::Other("something odd".into()));
        assert!(!friendly(OpFailure::TimedOut).is_empty());
    }

    #[test]
    fn parses_item_list() {
        let raw = r#"[{"id":"i1","title":"Beelink","vault":{"id":"v1","name":"Homelab"},
                      "additional_information":"deploy","urls":[{"href":"ssh://10.0.0.20"}]},
                     {"id":"i2","title":"Key","vault":{"id":"v1","name":"Homelab"}}]"#;
        let items: Vec<RawItem> = serde_json::from_str(raw).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].additional_information.as_deref(), Some("deploy"));
    }

    /// Against the real 1Password (may ask for approval): every account's SSH
    /// keys can be fetched and match their listed fingerprint. Prints only
    /// counts and whether fingerprints match, never key material.
    #[tokio::test]
    #[ignore]
    async fn live_ssh_keys_match_fingerprints() {
        for account in accounts().await.unwrap() {
            let vaults = vaults(Some(&account.id)).await.unwrap();
            let keys = ssh_keys(Some(&account.id), None).await.unwrap();
            eprintln!("OP account {} · {} vault(s) · {} SSH key(s)", account.url, vaults.len(), keys.len());
            if let Some(k) = keys.first() {
                let pem = read_ssh_key(&k.item, Some(&account.id)).await.unwrap();
                let key = russh::keys::decode_secret_key(&pem, None).unwrap();
                let fp = crate::ssh::fingerprint(key.public_key());
                eprintln!("OP   first key: fingerprint matches = {}", fp == k.fingerprint);
                assert_eq!(fp, k.fingerprint);
            }
        }
    }
}
