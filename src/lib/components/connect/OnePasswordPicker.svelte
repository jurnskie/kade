<script lang="ts">
  import type { OpItem, OpSshKey } from "$lib/api";
  import { t } from "$lib/i18n.svelte";
  import OnePasswordIcon from "../OnePasswordIcon.svelte";
  import PickList, { type PickRow } from "./PickList.svelte";
  import type { OnePassword } from "./onepassword.svelte";

  let {
    op,
    listing,
    pinned = $bindable(),
    keyItem = $bindable(),
    reference = $bindable(),
    user = $bindable(),
  }: {
    op: OnePassword;
    /** An SSH key to sign in with, or a login whose password is used. */
    listing: "keys" | "logins";
    pinned: string | null;
    keyItem: string | null;
    reference: string;
    user: string;
  } = $props();

  function pickKey(k: OpSshKey) {
    pinned = k.fingerprint;
    keyItem = k.item;
  }

  function pickLogin(item: OpItem) {
    reference = item.reference;
    if (!user.trim() && item.username) user = item.username;
  }

  const LIMIT = 40;
  const matches = $derived.by(() => {
    const q = op.query.trim().toLowerCase();
    const items = op.items ?? [];
    return q
      ? items.filter((i) => `${i.title} ${i.username ?? ""} ${i.url ?? ""} ${i.vault}`.toLowerCase().includes(q))
      : items;
  });
  const logins = $derived(matches.slice(0, LIMIT));
  const wsKey = $derived(op.defaultKey);
  // While the key list is deferred, the title saved with the workspace stands in; older workspaces only have the fingerprint.
  const wsKeyTitle = $derived.by(() => {
    const key = op.workspaceKey;
    if (!key) return "";
    return op.keys?.find((k) => k.fingerprint === key.fingerprint)?.title ?? key.title ?? key.fingerprint.replace("SHA256:", "").slice(0, 12) + "…";
  });
  const selected = $derived(op.items?.find((i) => i.reference === reference) ?? null);

  const rows: PickRow[] = $derived(
    listing === "keys"
      ? (op.keys ?? []).map((k) => ({
          id: k.item,
          title: k.title,
          detail: `${k.vault} · ${k.fingerprint.replace("SHA256:", "").slice(0, 12)}…`,
          mono: true,
          on: keyItem === k.item || (!keyItem && pinned === k.fingerprint),
          pick: () => pickKey(k),
        }))
      : logins.map((item) => ({
          id: item.reference,
          title: item.title,
          detail: `${item.vault}${item.username ? ` · ${item.username}` : ""}${item.url ? ` · ${item.url}` : ""}`,
          on: reference === item.reference,
          pick: () => pickLogin(item),
        })),
  );
</script>

{#if !(listing === "keys" && op.usesDefaultKey)}
<div class="scope">
  {#if (op.accounts?.length ?? 0) > 1}
    <select bind:value={op.account} title={t("1Password account")} onchange={() => (op.picked = true)}>
      {#each op.accounts ?? [] as a (a.id)}<option value={a.id}>{a.url}{a.email ? ` · ${a.email}` : ""}</option>{/each}
    </select>
  {:else if op.accounts?.length === 1}
    <span class="acct" title={op.accounts[0].email}>{op.accounts[0].url}</span>
  {/if}
  <select bind:value={op.vault} title={t("Vault")} onchange={() => (op.vaultPicked = true)}>
    <option value="">{t("All vaults")}</option>
    {#each op.vaults as v (v.id)}<option value={v.id}>{v.name}</option>{/each}
  </select>
</div>
{/if}

{#if listing === "keys" && op.usesDefaultKey}
  <p class="ws-key"><OnePasswordIcon size={13} />{t("Uses the workspace's default key: {key}", { key: wsKeyTitle })}</p>
  <button class="btn another" onclick={() => op.browse()}>{t("Choose another key")}</button>
{:else if listing === "keys"}
  {#if wsKey && !pinned && !keyItem}
    <p class="ws-key"><OnePasswordIcon size={13} />{t("Uses the workspace's default key: {key}", { key: wsKeyTitle })}</p>
  {/if}
  <PickList
    {rows}
    loading={op.loading ? t("Fetching keys…") : null}
    error={op.error}
    onretry={() => op.retry()}
    empty={t("No SSH keys in this vault.")}
  />
  <p class="hint">
    {t("Kade tries the 1Password SSH agent first. If it doesn't offer the key on this computer (e.g. a work laptop), Kade fetches it from this account and vault.")}
  </p>
{:else}
  {#if selected}
    <div class="picked">
      <OnePasswordIcon size={14} />
      <span><b>{selected.title}</b><small>{selected.vault}{selected.username ? ` · ${selected.username}` : ""}</small></span>
    </div>
  {:else if reference}
    <div class="picked"><OnePasswordIcon size={14} /><span class="mono small">{reference}</span></div>
  {/if}
  <div class="in"><input bind:value={op.query} placeholder={t("Search logins in 1Password…")} spellcheck="false" /></div>
  <PickList
    {rows}
    loading={op.loading ? t("Fetching logins… (1Password may ask for approval)") : null}
    error={op.error}
    onretry={() => op.retry()}
    empty={t("No logins found.")}
  />
  {#if matches.length > LIMIT}
    <p class="hint">{t("Showing {shown} of {n} — search to narrow down", { shown: LIMIT, n: matches.length })}</p>
  {/if}
{/if}

<style>
  .scope {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
  }
  .scope select {
    height: 30px;
    border: 1px solid var(--mist);
    border-radius: 7px;
    padding: 0 8px;
    background: var(--paper);
    font-size: 12px;
    max-width: 100%;
  }
  .acct {
    font-size: 12px;
    color: var(--ink2);
    padding: 0 4px;
  }
  .picked {
    display: flex;
    gap: 9px;
    align-items: center;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--pine-t);
  }
  .picked b {
    display: block;
    font-weight: 600;
    font-size: 12.5px;
  }
  .picked small {
    display: block;
    font-size: 11px;
    color: var(--ink2);
  }
  .ws-key {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 12px;
    color: var(--ink2);
  }
  .another {
    align-self: flex-start;
  }
  .small {
    font-size: 11px;
    word-break: break-all;
  }
</style>
