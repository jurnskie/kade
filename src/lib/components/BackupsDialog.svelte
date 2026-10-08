<script lang="ts">
  import { onMount } from "svelte";
  import { ArchiveRestore, Trash2, Laptop, Server, LoaderCircle, Undo2 } from "@lucide/svelte";
  import { api, errorMessage, type Transaction } from "$lib/api";
  import { locale, t, tn } from "$lib/i18n.svelte";
  import Modal from "./Modal.svelte";

  let {
    retentionDays,
    sessionFor,
    onrestored,
    onclose,
  }: {
    retentionDays: number;
    /** Session id of an open tab for this server, if any. */
    sessionFor: (serverId: string | null) => string | null;
    onrestored: (tx: Transaction) => void;
    onclose: () => void;
  } = $props();

  let txs = $state<Transaction[]>([]);
  let loading = $state(true);
  let busy = $state<string | null>(null);
  let error = $state<string | null>(null);
  let confirmDelete = $state<string | null>(null);

  async function reload() {
    try {
      txs = await api.backupsList();
    } finally {
      loading = false;
    }
  }
  onMount(() => void reload().catch((e) => (error = errorMessage(e))));

  const timeFmt = $derived(new Intl.DateTimeFormat(locale(), { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" }));

  function expires(tx: Transaction) {
    const left = Math.ceil((tx.created + retentionDays * 86_400_000 - Date.now()) / 86_400_000);
    return left <= 0 ? t("expires at the next clean-up") : tn(left, "kept for {n} more day", "kept for {n} more days");
  }

  /** Server-side data needs that server's connection; local data never does. */
  function needsSession(tx: Transaction) {
    return tx.side === "remote";
  }

  function sessionOf(tx: Transaction) {
    return needsSession(tx) ? sessionFor(tx.server_id) : null;
  }

  async function restore(tx: Transaction) {
    busy = tx.id;
    error = null;
    try {
      await api.backupRestore(tx.id, sessionOf(tx));
      onrestored(tx);
      await reload();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = null;
    }
  }

  async function remove(tx: Transaction) {
    busy = tx.id;
    error = null;
    try {
      const storedRemotely = tx.entries.some((e) => e.stored_on === "remote");
      await api.backupDelete(tx.id, storedRemotely ? sessionOf(tx) : null);
      confirmDelete = null;
      await reload();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = null;
    }
  }

  const opLabel = $derived({ delete: t("Deleted"), overwrite: t("Overwritten"), restore: t("Before restore") });
</script>

<Modal width={760} labelledby="bk-title" {onclose}>
  {#snippet header(close)}
    <div class="sh">
      <ArchiveRestore size={18} color="var(--pine)" />
      <div>
        <h1 id="bk-title">Backups</h1>
        <p>{tn(retentionDays, "Everything Kade deletes or overwrites is kept for {n} day.", "Everything Kade deletes or overwrites is kept for {n} days.")}</p>
      </div>
      {@render close()}
    </div>
  {/snippet}

  {#if error}<div class="err">{error}</div>{/if}

  <div class="list">
    {#if loading}
      <div class="empty"><LoaderCircle size={18} class="spin" /></div>
    {:else if txs.length === 0}
      <div class="empty">{t("No backups yet. As soon as you delete or overwrite something, it shows up here.")}</div>
    {/if}
    {#each txs as tx (tx.id)}
      {@const offline = needsSession(tx) && !sessionOf(tx)}
      <div class="tx" class:done={tx.restored}>
        <div class="ic">
          {#if tx.side === "remote"}<Server size={16} color="var(--ink2)" />{:else}<Laptop size={16} color="var(--ink2)" />{/if}
        </div>
        <div class="t">
          <div class="top">
            <span class="op" class:ov={tx.op !== "delete"}>{opLabel[tx.op]}</span>
            <b>{tx.summary}</b>
          </div>
          <small>
            {timeFmt.format(new Date(tx.created))} · {tx.server_name ?? t("this computer")} ·
            {tn(tx.entries.length, "{n} item", "{n} items")} ·
            {tx.restored ? t("restored") : expires(tx)}
          </small>
          {#if !tx.restored && tx.entries.some((e) => e.created)}
            {@const added = tx.entries.filter((e) => e.created).length}
            {@const back = tx.entries.length - added}
            <small class="hint">
              {back > 0
                ? t("Restore puts {back} replaced or deleted back and moves {added} added aside. Both can be undone.", { back, added })
                : t("Restore moves the {added} added aside. That can be undone.", { added })}
            </small>
          {/if}
          <small class="mono paths" title={tx.entries.map((e) => e.original).join("\n")}>
            {tx.entries.slice(0, 3).map((e) => e.original).join(", ")}{tx.entries.length > 3 ? ", …" : ""}
          </small>
        </div>
        <div class="acts">
          {#if confirmDelete === tx.id}
            <button class="btn ghost" onclick={() => (confirmDelete = null)}>{t("No")}</button>
            <button class="btn danger-fill" disabled={busy === tx.id} onclick={() => remove(tx)}>{t("Delete permanently")}</button>
          {:else}
            <button
              class="btn"
              disabled={busy != null || offline}
              title={offline ? t("Connect to {name} first", { name: tx.server_name ?? "" }) : t("Put the items back where they were")}
              onclick={() => restore(tx)}
            >
              {#if busy === tx.id}<LoaderCircle size={14} class="spin" />{:else}<Undo2 size={14} />{/if}
              {t("Restore")}
            </button>
            <button
              class="btn ghost icon"
              title={offline ? t("Connect to {name} first", { name: tx.server_name ?? "" }) : t("Delete backup")}
              disabled={busy != null || (offline && tx.entries.some((e) => e.stored_on === "remote"))}
              onclick={() => (confirmDelete = tx.id)}
            >
              <Trash2 size={14} />
            </button>
          {/if}
        </div>
      </div>
    {/each}
  </div>
</Modal>

<style>
  .sh {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 18px 22px 14px;
    border-bottom: 1px solid var(--mist2);
  }
  .sh :global(svg) {
    margin-top: 3px;
  }
  h1 {
    font-size: 18px;
    font-weight: 600;
  }
  .sh p {
    color: var(--lichen);
    margin-top: 2px;
  }
  .err {
    margin: 12px 22px 0;
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--danger-t);
    color: var(--danger);
    font-size: 12.5px;
  }
  .list {
    overflow-y: auto;
    padding: 8px 12px 14px;
  }
  .empty {
    padding: 40px 20px;
    text-align: center;
    color: var(--lichen);
  }
  .tx {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    padding: 12px 10px;
    border-radius: 10px;
  }
  .tx + .tx {
    border-top: 1px solid var(--mist2);
  }
  .tx.done {
    opacity: 0.6;
  }
  .ic {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    background: var(--snow);
    border: 1px solid var(--mist);
    display: grid;
    place-items: center;
    flex: none;
  }
  .t {
    flex: 1;
    min-width: 0;
  }
  .top {
    display: flex;
    gap: 8px;
    align-items: center;
    min-width: 0;
  }
  .top b {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .op {
    font: 500 10.5px var(--mono);
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--danger-t);
    color: var(--danger);
    flex: none;
  }
  .op.ov {
    background: var(--amber-t);
    color: var(--amber);
  }
  small {
    display: block;
    color: var(--lichen);
    font-size: 11.5px;
    margin-top: 3px;
  }
  .paths {
    font-size: 11px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .acts {
    display: flex;
    gap: 6px;
    flex: none;
    align-items: center;
  }
  .btn.icon {
    padding: 7px 8px;
  }
  .danger-fill {
    background: var(--danger);
    border-color: var(--danger);
    color: var(--on-danger);
  }
  .danger-fill:hover {
    background: var(--danger-hover);
  }
</style>
