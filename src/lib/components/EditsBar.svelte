<script lang="ts">
  import { FilePen, X, LoaderCircle, CircleCheck, TriangleAlert, Upload, Download, ExternalLink } from "@lucide/svelte";
  import { api, type EditInfo } from "$lib/api";
  import { edits } from "$lib/edits.svelte";
  import { locale, t } from "$lib/i18n.svelte";

  let { sessionId, onerror }: { sessionId: string; onerror: (e: unknown) => void } = $props();

  const mine = $derived(edits.items.filter((e) => e.session_id === sessionId));
  const timeFmt = $derived(new Intl.DateTimeFormat(locale(), { hour: "2-digit", minute: "2-digit", second: "2-digit" }));

  function status(e: EditInfo) {
    switch (e.state) {
      case "uploading":
        return t("uploading…");
      case "uploaded":
        return t("saved to server · {time}", { time: timeFmt.format(new Date(e.at)) });
      case "conflict":
        return e.message ?? t("conflict");
      case "error":
        return e.message ?? t("error");
      default:
        return e.message ?? (e.uploads ? t("saved {n}×", { n: e.uploads }) : t("watching · save in your editor"));
    }
  }

  const act = (p: Promise<unknown>) => p.catch(onerror);
</script>

{#if mine.length}
  <div class="bar">
    {#each mine as e (e.id)}
      <div class="ed" class:conflict={e.state === "conflict"} class:error={e.state === "error"} title={e.remote_path}>
        <span class="ic">
          {#if e.state === "uploading"}<LoaderCircle size={15} class="spin" color="var(--pine)" />
          {:else if e.state === "uploaded"}<CircleCheck size={15} color="var(--pine)" />
          {:else if e.state === "conflict" || e.state === "error"}<TriangleAlert size={15} color="currentColor" />
          {:else}<FilePen size={15} color="var(--ink2)" />{/if}
        </span>
        <span class="t"><b>{e.name}</b><small>{status(e)}</small></span>
        {#if e.state === "conflict"}
          <button class="btn sm" onclick={() => act(api.editForce(e.id))}><Upload size={13} />{t("Upload anyway")}</button>
          <button class="btn sm" onclick={() => act(api.editReload(e.id))}><Download size={13} />{t("Get server version")}</button>
        {:else}
          <button class="ic-btn" title={t("Open in editor again")} onclick={() => act(api.editOpen(e.session_id, e.remote_path))}>
            <ExternalLink size={14} />
          </button>
        {/if}
        <button class="ic-btn" title={t("Stop watching")} onclick={() => act(edits.stop(e.id))}><X size={14} /></button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .bar {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    padding: 0 18px 10px;
  }
  .ed {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
    max-width: 100%;
    padding: 6px 6px 6px 10px;
    border-radius: 10px;
    background: var(--paper);
    border: 1px solid var(--mist);
  }
  .ed.conflict {
    background: var(--amber-t);
    border-color: var(--amber-line);
    color: var(--amber-ink);
  }
  .ed.error {
    background: var(--danger-t);
    border-color: var(--danger-line);
    color: var(--danger);
  }
  .ic {
    display: grid;
    flex: none;
  }
  .t {
    min-width: 0;
  }
  .t b {
    display: block;
    font-weight: 600;
    font-size: 12.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .t small {
    display: block;
    font-size: 11px;
    color: var(--lichen);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .conflict .t small,
  .error .t small {
    color: inherit;
  }
  .btn.sm {
    padding: 4px 8px;
    font-size: 12px;
    flex: none;
  }
  .ic-btn {
    display: grid;
    padding: 4px;
    border-radius: 6px;
    color: var(--ink2);
    flex: none;
  }
  .ic-btn:hover {
    background: var(--mist2);
  }
</style>
