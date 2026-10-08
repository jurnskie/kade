<script lang="ts">
    import { Layers, Trash2, Check } from "@lucide/svelte";
  import { api, errorMessage, type OpAccount, type OpSshKey, type OpVault, type Workspace } from "$lib/api";
  import { WORKSPACE_COLORS } from "$lib/workspaces";
  import OnePasswordIcon from "./OnePasswordIcon.svelte";
  import Modal from "./Modal.svelte";
  import { t, tn } from "$lib/i18n.svelte";

  let {
    initial,
    workspaces,
    counts,
    onsaved,
    ondeleted,
    onclose,
  }: {
    initial: Workspace | null;
    workspaces: Workspace[];
    /** Connections per workspace id. */
    counts: Record<string, number>;
    onsaved: (ws: Workspace) => void;
    ondeleted: (movedTo: string) => void;
    onclose: () => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  const start = initial;
  let name = $state(start?.name ?? "");
  // svelte-ignore state_referenced_locally
  let color = $state(start?.color ?? Object.keys(WORKSPACE_COLORS)[workspaces.length % 6]);
  let opAccount = $state(start?.op_account ?? "");
  let accounts = $state<OpAccount[]>([]);
  // `op` runs one call at a time: accounts first, then vaults, then keys.
  let accountsReady = $state(false);
  let attempt = $state(0);
  let accountsError = $state<string | null>(null);
  let vaultsError = $state<string | null>(null);
  let keysError = $state<string | null>(null);
  const opError = $derived(accountsError ?? vaultsError ?? keysError);
  let opVault = $state(start?.op_vault ?? "");
  // The key's item identifies it in the select; its fingerprint is what gets pinned.
  let keyItem = $state(start?.op_key_item ?? "");
  let keyFingerprint = $state(start?.op_key_fingerprint ?? "");
  let vaults = $state<OpVault[]>([]);
  let keys = $state<OpSshKey[]>([]);
  let keysLoading = $state(false);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let deleting = $state(false);
  // svelte-ignore state_referenced_locally
  let moveTo = $state(workspaces.find((w) => w.id !== start?.id)?.id ?? "");

  $effect(() => {
    void attempt;
    if (accountsReady) return;
    accountsError = null;
    let cancelled = false;
    api
      .opAccounts()
      .then((a) => {
        if (cancelled) return;
        accounts = a;
        accountsReady = true;
      })
      .catch((e) => !cancelled && (accountsError = errorMessage(e)));
    return () => (cancelled = true);
  });

  // Vaults of the chosen account, once the accounts are known.
  $effect(() => {
    const account = opAccount || null;
    void attempt;
    if (!accountsReady) return;
    vaultsError = null;
    let cancelled = false;
    api
      .opVaults(account)
      .then((v) => !cancelled && (vaults = v))
      .catch((e) => {
        if (cancelled) return;
        vaults = [];
        vaultsError = errorMessage(e);
      });
    return () => (cancelled = true);
  });

  // SSH keys of the chosen vault, once there is one.
  $effect(() => {
    const [account, vault] = [opAccount || null, opVault];
    void attempt;
    keys = [];
    keysLoading = false;
    keysError = null;
    if (!vault) return;
    keysLoading = true;
    let cancelled = false;
    api
      .opSshKeys(account, vault)
      .then((k) => !cancelled && (keys = k))
      .catch((e) => {
        if (cancelled) return;
        keys = [];
        keysError = errorMessage(e);
      })
      .finally(() => !cancelled && (keysLoading = false));
    return () => (cancelled = true);
  });

  function accountChanged() {
    opVault = "";
    vaultChanged();
  }

  function vaultChanged() {
    keyItem = "";
    keyFingerprint = "";
  }

  function keyChanged() {
    keyFingerprint = keys.find((k) => k.item === keyItem)?.fingerprint ?? "";
  }

  const others = $derived(workspaces.filter((w) => w.id !== start?.id));
  const count = $derived(start ? (counts[start.id] ?? 0) : 0);

  async function save() {
    if (busy || !name.trim()) return;
    busy = true;
    error = null;
    try {
      const ws = await api.saveWorkspace({
        id: start?.id ?? "",
        name,
        color,
        op_account: opAccount || null,
        op_vault: opVault || null,
        op_key_fingerprint: opVault && keyItem ? keyFingerprint || null : null,
        op_key_item: opVault && keyItem ? keyItem : null,
        op_key_title: opVault && keyItem ? (keys.find((k) => k.item === keyItem)?.title ?? start?.op_key_title ?? null) : null,
        updated_at: 0,
      });
      onsaved(ws);
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }

  async function remove() {
    if (!start) return;
    busy = true;
    try {
      await api.deleteWorkspace(start.id, moveTo);
      ondeleted(moveTo);
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
</script>

<Modal width={440} z={20} pad="18px 20px 16px" gap={14} labelledby="ws-title" {onclose}>
  {#snippet header(close)}
    <div class="sh">
      <Layers size={18} color="var(--pine)" />
      <h1 id="ws-title">{start ? t("Edit workspace") : t("New workspace")}</h1>
      {@render close()}
    </div>
  {/snippet}

  <label class="f">
    <span>{t("Name")}</span>
    <!-- svelte-ignore a11y_autofocus -->
    <input bind:value={name} placeholder={t("Work")} autofocus onkeydown={(e) => e.key === "Enter" && (e.preventDefault(), save())} />
  </label>

  <div class="f">
    <span>{t("Colour")}</span>
    <div class="swatches">
      {#each Object.entries(WORKSPACE_COLORS) as [key, c] (key)}
        <button
          class="sw"
          class:on={color === key}
          style:background={c.color}
          title={t(c.label)}
          aria-label={t(c.label)}
          onclick={() => (color = key)}
        >
          {#if color === key}<Check size={13} color={c.onColor} />{/if}
        </button>
      {/each}
    </div>
  </div>

  <label class="f">
    <span><OnePasswordIcon size={13} /> {t("Default 1Password account")}</span>
    <select bind:value={opAccount} onchange={accountChanged}>
      <option value="">{t("1Password's default account")}</option>
      {#each accounts as a (a.id)}<option value={a.id}>{a.url}{a.email ? ` · ${a.email}` : ""}</option>{/each}
    </select>
    <small>{t("Connections in this workspace use this account unless they chose one themselves.")}</small>
  </label>

  <label class="f">
    <span>{t("Default vault")}</span>
    <select bind:value={opVault} onchange={vaultChanged}>
      <option value="">{t("All vaults")}</option>
      {#each vaults as v (v.id)}<option value={v.id}>{v.name}</option>{/each}
    </select>
    <small>{t("The key list of connections in this workspace starts in this vault.")}</small>
  </label>

  {#if opVault}
    <label class="f">
      <span>{t("Default SSH key")}</span>
      <select bind:value={keyItem} onchange={keyChanged} disabled={keysLoading}>
        <option value="">{keysLoading ? t("Fetching keys…") : t("None — choose per connection")}</option>
        {#each keys as k (k.item)}<option value={k.item}>{k.title}</option>{/each}
      </select>
      <small>
        {t("Connections without a key of their own use only this key. Without a default key, only keys from the vault are tried.")}
      </small>
    </label>
  {/if}

  {#if opError}
    <p class="err op">
      <span>{opError}</span>
      <button class="btn ghost" onclick={() => attempt++}>{t("Retry")}</button>
    </p>
  {/if}
  {#if error}<p class="err">{error}</p>{/if}

  {#if deleting && start}
    <div class="del">
      <p>
        {t("Delete {name}?", { name: start.name })}
        {#if count}{tn(count, "Its {n} connection moves to:", "Its {n} connections move to:")}{/if}
      </p>
      {#if count}
        <select bind:value={moveTo}>
          {#each others as w (w.id)}<option value={w.id}>{w.name}</option>{/each}
        </select>
      {/if}
      <div class="actions">
        <button class="btn ghost" onclick={() => (deleting = false)}>{t("No")}</button>
        <button class="btn danger-fill" disabled={busy || !moveTo} onclick={remove}>{t("Delete")}</button>
      </div>
    </div>
  {:else}
    <div class="actions">
      {#if start && others.length}
        <button class="btn ghost danger" onclick={() => (deleting = true)}><Trash2 size={14} />{t("Delete")}</button>
      {/if}
      <span class="spacer"></span>
      <button class="btn ghost" onclick={onclose}>{t("Cancel")}</button>
      <button class="btn pri" disabled={busy || !name.trim()} onclick={save}>{start ? t("Save") : t("Create")}</button>
    </div>
  {/if}
</Modal>

<style>
  .sh {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  h1 {
    font-size: 16px;
    font-weight: 600;
  }
  .f {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .f > span {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 500;
    font-size: 12.5px;
    color: var(--ink2);
  }
  .f small {
    font-size: 11.5px;
    color: var(--lichen);
  }
  input,
  select {
    height: 36px;
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 0 10px;
    background: var(--paper);
    outline: 0;
  }
  input:focus,
  select:focus {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
  }
  .swatches {
    display: flex;
    gap: 8px;
  }
  .sw {
    width: 28px;
    height: 28px;
    border-radius: 8px;
    display: grid;
    place-items: center;
  }
  .sw.on {
    box-shadow: 0 0 0 2px var(--paper), 0 0 0 4px currentColor;
    outline: 2px solid var(--granite);
    outline-offset: 2px;
  }
  .err {
    color: var(--danger);
    font-size: 12.5px;
  }
  .err.op {
    display: flex;
    gap: 8px;
    align-items: center;
    font-size: 12px;
  }
  .err.op span {
    flex: 1;
  }
  .actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .spacer {
    flex: 1;
  }
  .del {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    border-radius: 10px;
    background: var(--danger-t);
  }
  .del .actions {
    justify-content: flex-end;
  }
  .btn.danger {
    color: var(--danger);
  }
  .danger-fill {
    background: var(--danger);
    border-color: var(--danger);
    color: var(--on-danger);
  }
</style>
