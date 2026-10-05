<script lang="ts">
  import { ArrowUp, ArrowDown, ArrowUpDown, Pause, Play, X, CircleCheck, TriangleAlert, Ban, ChevronDown, ChevronUp, ArchiveRestore, LoaderCircle } from "@lucide/svelte";
  import { api, errorMessage, type Progress } from "$lib/api";
  import { formatSize } from "$lib/format";
  import { transfers, isFinished } from "$lib/transfers.svelte";
  import { t } from "$lib/i18n.svelte";

  let { sessionId, onrestore }: { sessionId: string; onrestore: (backupId: string) => void } = $props();

  let open = $state(true);
  const jobs = $derived(transfers.jobs.filter((j) => j.session_id === sessionId));
  const active = $derived(jobs.filter((j) => !isFinished(j)));
  const totals = $derived({
    files: active.reduce((n, j) => n + j.files_total, 0),
    bytes: active.reduce((n, j) => n + j.bytes_total, 0),
  });

  function pct(j: Progress) {
    if (j.state === "done") return 100;
    return j.bytes_total ? Math.min(100, (j.bytes_done / j.bytes_total) * 100) : 0;
  }

  function eta(j: Progress) {
    if (!j.speed || !j.bytes_total) return "";
    const s = Math.round((j.bytes_total - j.bytes_done) / j.speed);
    return s < 60 ? t("{s} s left", { s }) : t("{m} min left", { m: Math.round(s / 60) });
  }

  function stateLabel(j: Progress) {
    switch (j.state) {
      case "queued":
        return t("queued");
      case "scanning":
        return t("scanning…");
      case "paused":
        return t("paused");
      case "done":
        return j.skipped ? t("done · {n} skipped", { n: j.skipped }) : t("done");
      case "failed":
        return t("failed");
      case "cancelled":
        return t("cancelled");
      default:
        return eta(j);
    }
  }

  const act = (p: Promise<unknown>) => p.catch((e) => console.error(errorMessage(e)));
</script>

{#if jobs.length}
  <div class="q">
    <div class="qh">
      <ArrowUpDown size={16} color="var(--pine)" />
      <b>{t("Transfers")}</b>
      <span class="meta">
        {#if active.length}{t("{n} active · {files} files · {size}", { n: active.length, files: totals.files, size: formatSize(totals.bytes) })}{:else}{t("all done")}{/if}
      </span>
      <div class="r">
        {#if active.length}
          <button onclick={() => active.forEach((j) => act(api.transferPause(j.id, j.state !== "paused")))}>
            {active.every((j) => j.state === "paused") ? t("Resume all") : t("Pause all")}
          </button>
        {/if}
        {#if jobs.length > active.length}
          <button onclick={() => transfers.clearFinished()}>{t("Clear finished")}</button>
        {/if}
        <button class="ic" aria-label={open ? t("Collapse") : t("Expand")} onclick={() => (open = !open)}>
          {#if open}<ChevronDown size={16} />{:else}<ChevronUp size={16} />{/if}
        </button>
      </div>
    </div>
    {#if open}
      <div class="list">
        {#each jobs.toReversed() as j (j.id)}
          <div class="qr" class:fail={j.state === "failed"}>
            <span class="dir">
              {#if j.state === "done"}<CircleCheck size={16} color="var(--pine)" />
              {:else if j.state === "failed"}<TriangleAlert size={16} color="var(--danger)" />
              {:else if j.state === "cancelled"}<Ban size={16} color="var(--lichen)" />
              {:else if j.state === "scanning" || j.state === "queued"}<LoaderCircle size={16} class="spin" color="var(--lichen)" />
              {:else if j.direction === "upload"}<ArrowUp size={16} color="var(--pine)" />
              {:else}<ArrowDown size={16} color="var(--pine)" />{/if}
            </span>
            <div class="nm">
              <b>{j.name}</b>
              <small class="mono" title={j.error ?? j.dest}>
                {#if j.error}{j.error}{:else}{j.direction === "upload" ? "→" : "←"} {j.dest}{/if}
              </small>
            </div>
            <div class="track"><i style:width="{pct(j)}%" class:dim={isFinished(j)}></i></div>
            <div class="num mono">
              {#if j.state === "running"}{formatSize(j.speed)}/s{:else}{formatSize(j.bytes_done)}{/if}
            </div>
            <div class="st">{stateLabel(j)}</div>
            <div class="acts">
              {#if !isFinished(j)}
                <button class="ic" title={j.state === "paused" ? t("Resume") : t("Pause")} onclick={() => act(api.transferPause(j.id, j.state !== "paused"))}>
                  {#if j.state === "paused"}<Play size={15} />{:else}<Pause size={15} />{/if}
                </button>
                <button class="ic" title={t("Cancel")} onclick={() => act(api.transferCancel(j.id))}><X size={15} /></button>
              {:else if j.backup_id}
                <button class="ic" title={t("Restore overwritten files")} onclick={() => onrestore(j.backup_id!)}>
                  <ArchiveRestore size={15} />
                </button>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  .q {
    margin: 0 18px 16px;
    background: var(--paper);
    border: 1px solid var(--mist);
    border-radius: 12px;
    flex: none;
    container: queue / inline-size;
  }
  .qh {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 14px;
  }
  .qh b {
    font-weight: 600;
  }
  .meta {
    color: var(--lichen);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .r {
    margin-left: auto;
    display: flex;
    gap: 12px;
    align-items: center;
    color: var(--ink2);
    font-size: 12px;
    flex: none;
  }
  .r button:hover {
    color: var(--granite);
  }
  .ic {
    display: grid;
    padding: 3px;
    border-radius: 6px;
    color: var(--ink2);
  }
  .ic:hover {
    background: var(--mist2);
  }
  .list {
    max-height: 190px;
    overflow-y: auto;
    border-top: 1px solid var(--mist2);
  }
  .qr {
    display: grid;
    grid-template-columns: 22px minmax(0, 1.3fr) minmax(60px, 1.4fr) 84px 110px 54px;
    align-items: center;
    gap: 12px;
    padding: 8px 14px;
    font-size: 12.5px;
  }
  .qr + .qr {
    border-top: 1px solid var(--mist2);
  }
  .dir {
    display: grid;
  }
  .nm {
    min-width: 0;
  }
  .nm b {
    font-weight: 500;
    display: block;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .nm small {
    display: block;
    color: var(--lichen);
    font-size: 11px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .qr.fail .nm small {
    color: var(--danger);
  }
  .track {
    height: 5px;
    border-radius: 99px;
    background: var(--mist2);
    overflow: hidden;
  }
  .track i {
    display: block;
    height: 100%;
    background: var(--pine);
    border-radius: 99px;
    transition: width 0.15s linear;
  }
  .track i.dim {
    opacity: 0.35;
  }
  .num {
    font-size: 12px;
    color: var(--ink2);
    text-align: right;
  }
  .st {
    font-size: 12px;
    color: var(--lichen);
    text-align: right;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .acts {
    display: flex;
    gap: 2px;
    justify-content: flex-end;
  }
  @container queue (max-width: 620px) {
    .qr {
      grid-template-columns: 22px minmax(0, 1fr) 70px 54px;
    }
    .track,
    .num {
      display: none;
    }
  }
</style>
