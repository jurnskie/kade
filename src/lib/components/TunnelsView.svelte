<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { Cable, Plus, Pencil, Trash2, Copy, Check, ArrowRight, TriangleAlert } from "@lucide/svelte";
  import { api, errorMessage, type ServerProfile, type Tunnel, type TunnelState } from "$lib/api";
  import { t, tn } from "$lib/i18n.svelte";
  import { store } from "$lib/store.svelte";
  import Prompt from "./Prompt.svelte";

  let {
    sessionId,
    server,
    onsave,
    onerror,
  }: {
    sessionId: string;
    server: ServerProfile;
    /** Save the profile with changed tunnels; resolves to the saved profile. */
    onsave: (profile: ServerProfile) => Promise<ServerProfile | null>;
    onerror: (e: unknown) => void;
  } = $props();

  const PRESETS = [
    { name: "MySQL", port: 3306 },
    { name: "PostgreSQL", port: 5432 },
    { name: "Redis", port: 6379 },
    { name: "Web (8080)", port: 8080 },
  ];

  let running = $state<Record<string, TunnelState>>({});
  let busy = $state<string | null>(null);
  /** Ports stay empty (null) until typed in. */
  type TunnelForm = Omit<Tunnel, "local_port" | "remote_port"> & { local_port: number | null; remote_port: number | null };
  let form = $state<TunnelForm | null>(null);
  let formError = $state<string | null>(null);
  let copied = $state<string | null>(null);
  /** The tunnel waiting for the user to confirm its deletion. */
  let deleting = $state<Tunnel | null>(null);

  const tunnels = $derived(server.tunnels ?? []);

  async function refresh() {
    const list = await api.tunnelsList(sessionId);
    running = Object.fromEntries(list.map((s) => [s.tunnel_id, s]));
  }

  onMount(() => {
    refresh().catch(onerror);
    const off = listen<string>("tunnels-changed", ({ payload }) => {
      if (payload === sessionId) refresh().catch(() => {});
    });
    return () => void off.then((f) => f());
  });

  async function toggle(tun: Tunnel) {
    busy = tun.id;
    try {
      if (running[tun.id]) await api.tunnelStop(sessionId, tun.id);
      else await api.tunnelStart(sessionId, tun.id);
      await refresh();
    } catch (e) {
      onerror(e);
    } finally {
      busy = null;
    }
  }

  /** First free local port at or above `port` among this connection's tunnels. */
  function freePort(port: number, except?: string) {
    const taken = new Set(tunnels.filter((x) => x.id !== except).map((x) => x.local_port));
    while (taken.has(port)) port++;
    return port;
  }

  function startNew(preset?: { name: string; port: number }) {
    formError = null;
    form = {
      id: "",
      name: preset?.name ?? "",
      local_port: preset ? freePort(preset.port) : null,
      remote_host: "127.0.0.1",
      remote_port: preset?.port ?? null,
      auto_start: false,
    };
  }

  /** The profile as stored now; saving from the tab's copy would undo changes synced in meanwhile. */
  function currentProfile(): ServerProfile {
    const current = store.servers.find((s) => s.id === server.id);
    if (!current) throw { kind: "other", message: t("This connection no longer exists, so the tunnels can't be saved.") };
    return $state.snapshot(current);
  }

  async function saveForm() {
    if (!form) return;
    formError = null;
    const snap = $state.snapshot(form);
    const f: Tunnel = { ...snap, local_port: snap.local_port ?? 0, remote_port: snap.remote_port ?? 0 };
    try {
      const profile = currentProfile();
      const existing = profile.tunnels ?? [];
      const list = f.id ? existing.map((x) => (x.id === f.id ? f : x)) : [...existing, f];
      const saved = await onsave({ ...profile, tunnels: list });
      if (!saved) return;
      // A running tunnel picks up new ports after a restart.
      if (f.id && running[f.id]) {
        await api.tunnelStop(sessionId, f.id);
        await api.tunnelStart(sessionId, f.id).catch(onerror);
        await refresh();
      }
      form = null;
    } catch (e) {
      formError = errorMessage(e);
    }
  }

  async function remove(tun: Tunnel) {
    deleting = null;
    try {
      const profile = currentProfile();
      if (running[tun.id]) await api.tunnelStop(sessionId, tun.id);
      await onsave({ ...profile, tunnels: (profile.tunnels ?? []).filter((x) => x.id !== tun.id) });
      await refresh();
    } catch (e) {
      onerror(e);
    }
  }

  function label(tun: Tunnel) {
    return tun.name || `${tun.remote_host}:${tun.remote_port}`;
  }

  async function copy(tun: Tunnel) {
    await navigator.clipboard.writeText(`127.0.0.1:${tun.local_port}`).catch(() => {});
    copied = tun.id;
    setTimeout(() => (copied = null), 1500);
  }

  function statusText(tun: Tunnel) {
    const s = running[tun.id];
    if (!s) return t("Off");
    if (s.open > 0) return tn(s.open, "{n} connection", "{n} connections");
    return t("Listening");
  }

  const portOk = (p: number | null): p is number => p != null && Number.isInteger(p) && p >= 1 && p <= 65535;
  const formValid = $derived(!!form && portOk(form.local_port) && portOk(form.remote_port) && form.remote_host.trim() !== "");
</script>

<section class="tunnels">
  <header>
    <div class="ic"><Cable size={18} color="var(--pine)" /></div>
    <div class="id">
      <h2>{t("Tunnels")}</h2>
      <p>{t("Reach services on or behind {name} as if they ran on this computer.", { name: server.name })}</p>
    </div>
    {#if tunnels.length && !form}
      <button class="btn" onclick={() => startNew()}><Plus size={15} />{t("New tunnel")}</button>
    {/if}
  </header>

  {#if tunnels.length}
    <div class="list">
      {#each tunnels as tun (tun.id)}
        {@const on = !!running[tun.id]}
        <div class="row" class:on>
          <button
            class="switch"
            role="switch"
            aria-checked={on}
            aria-label={on ? t("Stop tunnel") : t("Start tunnel")}
            disabled={busy === tun.id}
            onclick={() => toggle(tun)}
          ><span></span></button>
          <div class="t">
            <b>{label(tun)}</b>
            <span class="route mono">
              127.0.0.1:{tun.local_port}<ArrowRight size={12} />{tun.remote_host}:{tun.remote_port}
            </span>
          </div>
          {#if tun.auto_start}<span class="tag">{t("Starts on connect")}</span>{/if}
          <span class="st" class:live={on}>{statusText(tun)}</span>
          <button class="ib" title={t("Copy address")} onclick={() => copy(tun)}>
            {#if copied === tun.id}<Check size={14} color="var(--pine)" />{:else}<Copy size={14} />{/if}
          </button>
          <button class="ib" title={t("Edit")} onclick={() => ((formError = null), (form = { ...tun }))}><Pencil size={14} /></button>
          <button class="ib" title={t("Delete")} onclick={() => (deleting = tun)}><Trash2 size={14} /></button>
        </div>
      {/each}
    </div>
  {:else if !form}
    <div class="empty">
      <p>{t("No tunnels yet. Forward a port on the server to this computer, for a database client or an admin page that only listens locally.")}</p>
      <div class="presets">
        {#each PRESETS as p (p.port)}
          <button class="btn" onclick={() => startNew(p)}><Plus size={14} />{p.name}</button>
        {/each}
        <button class="btn ghost" onclick={() => startNew()}>{t("Other…")}</button>
      </div>
    </div>
  {/if}

  {#if form}
    <form class="form" onsubmit={(e) => (e.preventDefault(), saveForm())}>
      <div class="fields">
        <label class="f name">
          <span>{t("Name")}</span>
          <input bind:value={form.name} placeholder={t("e.g. MySQL")} />
        </label>
        <label class="f">
          <span>{t("Local port")}</span>
          <input type="number" min="1" max="65535" placeholder="3307" bind:value={form.local_port} />
        </label>
        <span class="arrow"><ArrowRight size={16} color="var(--lichen)" /></span>
        <label class="f host">
          <span>{t("Host (from the server)")}</span>
          <input class="mono" bind:value={form.remote_host} spellcheck="false" />
        </label>
        <label class="f">
          <span>{t("Port")}</span>
          <input type="number" min="1" max="65535" placeholder="3306" bind:value={form.remote_port} />
        </label>
      </div>
      <label class="check"><input type="checkbox" bind:checked={form.auto_start} />{t("Start automatically when connecting")}</label>
      {#if form.local_port != null && form.local_port > 0 && form.local_port < 1024}
        <p class="hint">{t("Ports below 1024 usually need administrator rights; pick 1024 or higher.")}</p>
      {/if}
      {#if formError}<p class="err">{formError}</p>{/if}
      <div class="actions">
        <button type="button" class="btn ghost" onclick={() => (form = null)}>{t("Cancel")}</button>
        <button type="submit" class="btn pri" disabled={!formValid}>{form.id ? t("Save") : t("Add tunnel")}</button>
      </div>
    </form>
  {/if}
</section>

{#if deleting}
  {@const tun = deleting}
  <Prompt icon={TriangleAlert} color="var(--danger)" title={t("Delete tunnel “{name}”?", { name: label(tun) })} onclose={() => (deleting = null)}>
    {#snippet message()}
      {t("The tunnel is removed from this connection.")}
    {/snippet}
    {#snippet actions()}
      <button class="btn ghost" onclick={() => (deleting = null)}>{t("Cancel")}</button>
      <button class="btn pri del" onclick={() => remove(tun)}>{t("Delete")}</button>
    {/snippet}
  </Prompt>
{/if}

<style>
  .btn.del {
    background: var(--danger);
    border-color: var(--danger);
  }
  .tunnels {
    flex: 1;
    overflow-y: auto;
    padding: 0 18px 18px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 4px 2px 2px;
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
    flex: 1;
    min-width: 0;
  }
  h2 {
    font-size: 17px;
    font-weight: 600;
  }
  .id p {
    color: var(--lichen);
    font-size: 12px;
  }
  .list {
    background: var(--paper);
    border: 1px solid var(--mist);
    border-radius: 12px;
    overflow: hidden;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 11px 12px 11px 14px;
  }
  .row + .row {
    border-top: 1px solid var(--mist2);
  }
  .t {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .t b {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .route {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    color: var(--lichen);
    white-space: nowrap;
  }
  .row.on .route {
    color: var(--ink2);
  }
  .tag {
    font-size: 11px;
    padding: 2px 7px;
    border-radius: 4px;
    background: var(--mist2);
    color: var(--ink2);
    flex: none;
  }
  .st {
    font-size: 12px;
    color: var(--lichen);
    min-width: 90px;
    text-align: right;
    flex: none;
  }
  .st.live {
    color: var(--pine);
    font-weight: 600;
  }
  .ib {
    padding: 6px;
    border-radius: 6px;
    color: var(--ink2);
    display: grid;
    flex: none;
  }
  .ib:hover {
    background: var(--mist2);
  }
  .switch {
    width: 34px;
    height: 20px;
    border-radius: 99px;
    background: var(--control);
    position: relative;
    flex: none;
    transition: background 0.15s;
  }
  .switch span {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--paper);
    box-shadow: var(--shadow-sm);
    transition: transform 0.15s;
  }
  .switch[aria-checked="true"] {
    background: var(--pine);
  }
  .switch[aria-checked="true"] span {
    transform: translateX(14px);
  }
  .switch:disabled {
    opacity: 0.6;
  }
  .empty {
    background: var(--paper);
    border: 1px dashed var(--line-strong);
    border-radius: 12px;
    padding: 22px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    align-items: flex-start;
  }
  .empty p {
    color: var(--ink2);
    max-width: 560px;
    line-height: 1.5;
  }
  .presets {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .form {
    background: var(--paper);
    border: 1px solid var(--mist);
    border-radius: 12px;
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .fields {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 10px;
  }
  .f {
    display: flex;
    flex-direction: column;
    gap: 5px;
    width: 96px;
  }
  .f.name {
    flex: 1 1 100%;
    width: auto;
  }
  .f.host {
    flex: 1;
    min-width: 140px;
  }
  .f span {
    font-size: 12px;
    font-weight: 500;
    color: var(--ink2);
  }
  .arrow {
    height: 34px;
    display: grid;
    place-items: center;
  }
  input:not([type="checkbox"]) {
    height: 34px;
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 0 10px;
    background: var(--paper);
    outline: 0;
    width: 100%;
  }
  input:not([type="checkbox"]):focus {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--ink2);
  }
  .check input {
    accent-color: var(--pine);
  }
  .hint {
    font-size: 12px;
    color: var(--amber);
  }
  .err {
    font-size: 12.5px;
    color: var(--danger);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  @container main (max-width: 760px) {
    .tunnels {
      padding: 0 12px 12px;
    }
    .tag,
    .st {
      display: none;
    }
  }
</style>
