<script lang="ts">
  import { Cpu, MemoryStick, HardDrive, ListOrdered, LoaderCircle, Clock, Server } from "@lucide/svelte";
  import { api, errorMessage, type ServerStatus } from "$lib/api";
  import { formatSize } from "$lib/format";
  import { locale, t, tn } from "$lib/i18n.svelte";

  let { sessionId, visible }: { sessionId: string; visible: boolean } = $props();

  const INTERVAL = 5000;
  let status = $state<ServerStatus | null>(null);
  let error = $state<string | null>(null);
  let updated = $state<number | null>(null);

  // Poll only while the tab is on screen; one request at a time.
  $effect(() => {
    if (!visible) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const tick = async () => {
      try {
        status = await api.serverStatus(sessionId);
        error = null;
        updated = Date.now();
      } catch (e) {
        error = errorMessage(e);
      }
      if (!stopped) timer = setTimeout(tick, INTERVAL);
    };
    tick();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  });

  const pct = (used: number, total: number) => (total > 0 ? Math.min(100, (used / total) * 100) : 0);
  const level = (p: number) => (p >= 90 ? "crit" : p >= 75 ? "warn" : "ok");
  const num = $derived(new Intl.NumberFormat(locale(), { maximumFractionDigits: 2, minimumFractionDigits: 2 }));
  const one = $derived(new Intl.NumberFormat(locale(), { maximumFractionDigits: 1, minimumFractionDigits: 1 }));
  const whole = $derived(new Intl.NumberFormat(locale(), { maximumFractionDigits: 0 }));
  const timeFmt = $derived(new Intl.DateTimeFormat(locale(), { hour: "2-digit", minute: "2-digit", second: "2-digit" }));

  function uptime(secs: number) {
    const d = Math.floor(secs / 86400);
    const h = Math.floor((secs % 86400) / 3600);
    const m = Math.floor((secs % 3600) / 60);
    if (d > 0) return `${tn(d, "{n} day", "{n} days")}, ${t("{n} h", { n: h })}`;
    if (h > 0) return `${t("{n} h", { n: h })} ${t("{n} min", { n: m })}`;
    return t("{n} min", { n: m });
  }

  const memUsed = $derived(status?.mem_total != null && status.mem_available != null ? status.mem_total - status.mem_available : null);
  const swapUsed = $derived(status?.swap_total ? status.swap_total - (status.swap_free ?? 0) : null);
  /** Load relative to the number of cores, for the bar. */
  const loadPct = $derived(status?.load && status.cpus ? pct(status.load[0], status.cpus) : null);
</script>

<section class="status">
  {#if !status && !error}
    <div class="empty"><LoaderCircle size={18} class="spin" /></div>
  {:else if !status && error}
    <div class="empty err">{error}</div>
  {:else if status}
    <header>
      <div class="ic"><Server size={18} color="var(--pine)" /></div>
      <div class="id">
        <h2>{status.hostname ?? "—"}</h2>
        <p>{[status.os, status.kernel].filter(Boolean).join(" · ")}</p>
      </div>
      {#if status.uptime_secs != null}
        <span class="chip"><Clock size={13} />{t("Up {time}", { time: uptime(status.uptime_secs) })}</span>
      {/if}
      <span class="live" class:stale={!!error} title={error ?? ""}>
        <i></i>{error ? t("Can't refresh") : updated ? t("Updated {time}", { time: timeFmt.format(updated) }) : ""}
      </span>
    </header>

    <div class="grid">
      <div class="card">
        <div class="ct"><Cpu size={15} />{t("CPU")}</div>
        {#if status.cpu_percent != null}
          <div class="big">{whole.format(status.cpu_percent)}<small>%</small></div>
          <div class="bar {level(status.cpu_percent)}"><span style:width="{status.cpu_percent}%"></span></div>
        {:else}
          <div class="big muted">—</div>
        {/if}
        <dl>
          {#if status.cpus}<div><dt>{t("Cores")}</dt><dd>{status.cpus}</dd></div>{/if}
          {#if status.load}
            <div>
              <dt title={t("Average number of processes waiting for the CPU over 1, 5 and 15 minutes")}>{t("Load")}</dt>
              <dd class="mono" class:hot={loadPct != null && loadPct >= 100}>{status.load.map((l) => num.format(l)).join("  ")}</dd>
            </div>
          {/if}
        </dl>
      </div>

      <div class="card">
        <div class="ct"><MemoryStick size={15} />{t("Memory")}</div>
        {#if status.mem_total && memUsed != null}
          {@const p = pct(memUsed, status.mem_total)}
          <div class="big">{whole.format(p)}<small>%</small></div>
          <div class="bar {level(p)}"><span style:width="{p}%"></span></div>
          <dl>
            <div><dt>{t("In use")}</dt><dd>{formatSize(memUsed)} / {formatSize(status.mem_total)}</dd></div>
            {#if status.swap_total}
              <div><dt>{t("Swap")}</dt><dd>{formatSize(swapUsed ?? 0)} / {formatSize(status.swap_total)}</dd></div>
            {/if}
          </dl>
        {:else}
          <div class="big muted">—</div>
        {/if}
      </div>

      <div class="card wide">
        <div class="ct"><HardDrive size={15} />{t("Disks")}</div>
        {#each status.disks as d (d.mount)}
          {@const p = pct(d.used, d.size)}
          <div class="disk" title={d.filesystem}>
            <div class="dl">
              <span class="mono">{d.mount}</span>
              <span>{t("{free} free of {size}", { free: formatSize(d.available), size: formatSize(d.size) })}</span>
            </div>
            <div class="bar {level(p)}"><span style:width="{p}%"></span></div>
          </div>
        {:else}
          <p class="muted">{t("No disk information")}</p>
        {/each}
      </div>

      {#if status.processes.length}
        <div class="card wide">
          <div class="ct"><ListOrdered size={15} />{t("Busiest processes")}</div>
          <table>
            <thead><tr><th>{t("Process")}</th><th>PID</th><th>CPU</th><th>{t("Memory")}</th></tr></thead>
            <tbody>
              {#each status.processes as p (p.pid)}
                <tr><td class="mono">{p.command}</td><td class="mono">{p.pid}</td><td>{one.format(p.cpu)}%</td><td>{one.format(p.mem)}%</td></tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>
  {/if}
</section>

<style>
  .status {
    flex: 1;
    overflow-y: auto;
    padding: 0 18px 18px;
    -webkit-user-select: text;
    user-select: text;
  }
  .empty {
    display: grid;
    place-items: center;
    height: 100%;
    color: var(--lichen);
  }
  .empty.err {
    color: var(--danger);
    padding: 40px;
    text-align: center;
  }
  header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 4px 2px 14px;
    flex-wrap: wrap;
  }
  .ic {
    width: 36px;
    height: 36px;
    border-radius: 10px;
    background: var(--pine-t);
    display: grid;
    place-items: center;
    flex: none;
  }
  .id {
    min-width: 0;
    flex: 1;
  }
  h2 {
    font-size: 17px;
    font-weight: 600;
  }
  .id p {
    color: var(--lichen);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-radius: 99px;
    background: var(--mist2);
    color: var(--ink2);
    font-size: 12px;
    font-weight: 500;
  }
  .live {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    color: var(--lichen);
  }
  .live i {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--pine);
    animation: pulse 2s ease-in-out infinite;
  }
  .live.stale i {
    background: var(--amber);
    animation: none;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 12px;
  }
  .card {
    background: var(--paper);
    border: 1px solid var(--mist);
    border-radius: 12px;
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }
  .card.wide {
    grid-column: 1 / -1;
  }
  .ct {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 11px;
    font-weight: 600;
    color: var(--lichen);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .big {
    font-size: 30px;
    font-weight: 600;
    letter-spacing: -0.02em;
    line-height: 1;
  }
  .big small {
    font-size: 15px;
    color: var(--lichen);
    margin-left: 2px;
  }
  .muted {
    color: var(--lichen);
  }
  .bar {
    height: 6px;
    border-radius: 99px;
    background: var(--mist2);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--pine);
    transition: width 0.4s ease;
  }
  .bar.warn span {
    background: var(--amber);
  }
  .bar.crit span {
    background: var(--danger);
  }
  dl {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12.5px;
  }
  dl div {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }
  dt {
    color: var(--lichen);
  }
  dd {
    margin: 0;
    color: var(--ink2);
    white-space: nowrap;
  }
  dd.hot {
    color: var(--amber);
  }
  .disk {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .dl {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 12.5px;
  }
  .dl span:first-child {
    color: var(--granite);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dl span:last-child {
    color: var(--lichen);
    white-space: nowrap;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12.5px;
  }
  th {
    text-align: left;
    font-weight: 500;
    color: var(--lichen);
    padding: 0 8px 6px 0;
  }
  td {
    padding: 5px 8px 5px 0;
    border-top: 1px solid var(--mist2);
    color: var(--ink2);
  }
  th:not(:first-child),
  td:not(:first-child) {
    text-align: right;
    width: 80px;
  }
  td:first-child {
    color: var(--granite);
    max-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  @container main (max-width: 760px) {
    .status {
      padding: 0 12px 12px;
    }
    .grid {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
