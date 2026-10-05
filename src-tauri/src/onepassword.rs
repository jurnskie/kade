//! Secrets from 1Password via its CLI (`op`), with the desktop-app integration
//! doing the unlocking. Kade stores only references (`op://vault/item/…`) plus
//! the account they belong to: on a machine signed in to several accounts
//! (work and personal), `op` would otherwise pick its default account.

use serde::{Deserialize, Serialize};
use tokio::process::Command;

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

#[derive(Serialize)]
pub struct OpSshKey {
    pub title: String,
    pub vault: String,
    pub vault_id: String,
    /// `SHA256:…`, matching what the SSH agent reports.
    pub fingerprint: String,
    /// `op://<vault>/<item>`: where Kade fetches the key when the agent lacks it.
    pub item: String,
}

async fn op_raw(args: &[&str], account: Option<&str>) -> AppResult<String> {
    let mut cmd = crate::hostenv::async_command("op");
    cmd.args(args);
    if let Some(account) = account.filter(|a| !a.is_empty()) {
        cmd.args(["--account", account]);
    }
    let out = cmd.output().await.map_err(|e| AppError::AgentUnavailable {
        message: tr!("1Password CLI (op) not found: {e}", "1Password CLI (op) niet gevonden: {e}", e = e),
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(AppError::AuthFailed { message: format!("1Password: {err}") });
    }
    String::from_utf8(out.stdout).map_err(AppError::other)
}

pub async fn accounts() -> AppResult<Vec<OpAccount>> {
    let raw: Vec<RawAccount> = serde_json::from_str(&op_raw(&["account", "list", "--format", "json"], None).await?)?;
    Ok(raw.into_iter().map(|a| OpAccount { id: a.account_uuid, email: a.email, url: a.url }).collect())
}

pub async fn vaults(account: Option<&str>) -> AppResult<Vec<OpVault>> {
    let raw: Vec<RawVault> = serde_json::from_str(&op_raw(&["vault", "list", "--format", "json"], account).await?)?;
    let mut out: Vec<OpVault> = raw.into_iter().map(|v| OpVault { id: v.id, name: v.name }).collect();
    out.sort_by_key(|v| v.name.to_lowercase());
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
    out.sort_by_key(|i| i.title.to_lowercase());
    Ok(out)
}

/// SSH key items with their fingerprints (no private keys are read).
pub async fn ssh_keys(account: Option<&str>, vault: Option<&str>) -> AppResult<Vec<OpSshKey>> {
    let mut args = vec!["item", "list", "--categories", "SSH Key", "--format", "json"];
    if let Some(v) = vault.filter(|v| !v.is_empty()) {
        args.extend(["--vault", v]);
    }
    let listed: Vec<RawItem> = serde_json::from_str(&op_raw(&args, account).await?)?;
    let mut out = Vec::new();
    for item in listed {
        // Only the public fingerprint field; the private key stays in 1Password.
        let fp =
            op_raw(&["item", "get", &item.id, "--vault", &item.vault.id, "--fields", "fingerprint"], account).await.unwrap_or_default();
        out.push(OpSshKey {
            item: format!("op://{}/{}", item.vault.id, item.id),
            title: item.title,
            vault: item.vault.name,
            vault_id: item.vault.id,
            fingerprint: fp.trim().to_string(),
        });
    }
    out.sort_by_key(|k| k.title.to_lowercase());
    Ok(out)
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
                eprintln!("OP   eerste sleutel: vingerafdruk klopt = {}", fp == k.fingerprint);
                assert_eq!(fp, k.fingerprint);
            }
        }
    }
}
