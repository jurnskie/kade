<script lang="ts">
  import { onMount } from "svelte";
  import { Layers, Trash2, Check } from "@lucide/svelte";
  import { api, errorMessage, type OpAccount, type Workspace } from "$lib/api";
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
  let error = $state<string | null>(null);
  let busy = $state(false);
  let deleting = $state(false);
  // svelte-ignore state_referenced_locally
  let moveTo = $state(workspaces.find((w) => w.id !== start?.id)?.id ?? "");

  onMount(() => {
    api.opAccounts().then((a) => (accounts = a)).catch(() => (accounts = []));
  });

  const others = $derived(workspaces.filter((w) => w.id !== start?.id));
  const count = $derived(start ? (counts[start.id] ?? 0) : 0);

  async function save() {
    busy = true;
    error = null;
    try {
      const ws = await api.saveWorkspace({
        id: start?.id ?? "",
        name,
        color,
        op_account: opAccount || null,
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
    <input bind:value={name} placeholder={t("Work")} autofocus onkeydown={(e) => e.key === "Enter" && save()} />
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
          {#if color === key}<Check size={13} color="#fff" />{/if}
        </button>
      {/each}
    </div>
  </div>

  <label class="f">
    <span><OnePasswordIcon size={13} /> {t("Default 1Password account")}</span>
    <select bind:value={opAccount}>
      <option value="">{t("1Password's default account")}</option>
      {#each accounts as a (a.id)}<option value={a.id}>{a.url}{a.email ? ` · ${a.email}` : ""}</option>{/each}
    </select>
    <small>{t("Connections in this workspace use this account unless they chose one themselves.")}</small>
  </label>

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
