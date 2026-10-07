<script lang="ts">
  import { LoaderCircle, TriangleAlert, Undo2, X } from "@lucide/svelte";
  import { toasts } from "$lib/toasts.svelte";
  import { tabs } from "$lib/tabs.svelte";
  import { store } from "$lib/store.svelte";
  import { t } from "$lib/i18n.svelte";

  let { onundo, onupdate }: { onundo: (backupId: string) => void; onupdate: () => void } = $props();
</script>

{#if tabs.connecting}
  <div class="busy"><LoaderCircle size={14} class="spin" />{t("Connecting to {name}…", { name: store.servers.find((s) => s.id === tabs.connecting)?.name ?? "" })}</div>
{/if}

{#if toasts.notice && !toasts.undo && !toasts.error}
  <div class="toast undo" role="status">
    <span>{toasts.notice}</span>
    <button aria-label={t("Close")} onclick={() => (toasts.notice = null)}><X size={14} /></button>
  </div>
{:else if toasts.update && !toasts.undo && !toasts.error}
  <div class="toast undo" role="status">
    <span>{t("Kade {v} is available", { v: toasts.update })}</span>
    <button onclick={() => (onupdate(), (toasts.update = null))}>{t("Update")}</button>
    <button aria-label={t("Later")} onclick={() => (toasts.update = null)}><X size={14} /></button>
  </div>
{/if}

{#if toasts.undo && !toasts.error}
  <div class="toast undo" role="status">
    <span>{toasts.undo.tx.summary}</span>
    <button onclick={() => toasts.undo && onundo(toasts.undo.tx.id)}><Undo2 size={14} />{t("Undo")}</button>
  </div>
{/if}

{#if toasts.error}
  <div class="toast" role="alert"><TriangleAlert size={16} color="var(--on-danger)" />{toasts.error}</div>
{/if}

<style>
  .busy,
  .toast {
    position: fixed;
    bottom: 18px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 9px 14px;
    border-radius: 10px;
    font-weight: 500;
    font-size: 12.5px;
    z-index: 30;
    max-width: calc(100vw - 40px);
  }
  .busy {
    background: var(--paper);
    border: 1px solid var(--mist);
    box-shadow: var(--shadow-md);
  }
  .toast.undo {
    background: var(--inverse);
    color: var(--on-inverse);
  }
  .toast.undo button {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 6px;
    padding: 4px 9px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--on-inverse) 14%, transparent);
    color: var(--on-inverse);
    font-weight: 600;
  }
  .toast.undo button:hover {
    background: color-mix(in srgb, var(--on-inverse) 24%, transparent);
  }
  .toast {
    background: var(--danger);
    color: var(--on-danger);
    box-shadow: var(--shadow-md);
  }
</style>
