<script lang="ts">
  import { onMount } from "svelte";
  import { open as pickPath } from "@tauri-apps/plugin-dialog";
  import { Import, X, FileSearch, FolderOpen, ArrowLeft, TriangleAlert } from "@lucide/svelte";
  import { api, errorMessage, type Auth, type ImportFound, type ImportPreview, type ImportSource, type Workspace } from "$lib/api";
  import { t, tn } from "$lib/i18n.svelte";

  let {
    workspaces,
    defaultWorkspace,
    onimported,
    onclose,
  }: {
    workspaces: Workspace[];
    defaultWorkspace: Workspace | null;
    onimported: (workspace: string, count: number) => void;
    onclose: () => void;
  } = $props();

  const APPS: { id: ImportSource; label: string }[] = [
    { id: "cyberduck", label: "Cyberduck" },
    { id: "filezilla", label: "FileZilla" },
    { id: "transmit", label: "Transmit" },
  ];

  let source = $state<ImportSource>("cyberduck");
  let found = $state<ImportFound[]>([]);
  let preview = $state<ImportPreview | null>(null);
  let chosenPath = $state("");
  /** Indexes into preview.candidates that will be imported. */
  let picked = $state<Set<number>>(new Set());
  // svelte-ignore state_referenced_locally
  let workspace = $state(defaultWorkspace?.id ?? workspaces[0]?.id ?? "default");
  /** How SFTP connections without a key file sign in. */
  let sshAuth = $state<"password" | "one_password" | "agent">("password");
  let error = $state<string | null>(null);
  let busy = $state(false);

  onMount(async () => {
    found = await api.importDetect().catch(() => []);
    const first = found[0];
    if (first) source = first.source;
  });

  const label = $derived(APPS.find((a) => a.id === source)?.label ?? "");
  const here = $derived(found.filter((f) => f.source === source));
  const sshWithoutKey = $derived(
    preview?.candidates.some((c, i) => picked.has(i) && c.profile.protocol === "sftp" && c.profile.auth.method === "password") ?? false,
  );

  async function load(path: string) {
    busy = true;
    error = null;
    try {
      const p = await api.importPreview(source, path);
      chosenPath = path;
      preview = p;
      picked = new Set(p.candidates.flatMap((c, i) => (c.duplicate ? [] : [i])));
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }

  async function choose(directory: boolean) {
    const filters =
      source === "filezilla"
        ? [{ name: "FileZilla", extensions: ["xml"] }]
        : source === "cyberduck"
          ? [{ name: "Cyberduck", extensions: ["duck"] }]
          : [];
    const path = await pickPath({ directory, title: t("Choose a {app} file", { app: label }), filters: directory ? [] : filters });
    if (typeof path === "string") await load(path);
  }

  function toggle(i: number) {
    const next = new Set(picked);
    next.has(i) ? next.delete(i) : next.add(i);
    picked = next;
  }

  function toggleAll() {
    if (!preview) return;
    picked = picked.size === preview.candidates.length ? new Set() : new Set(preview.candidates.map((_, i) => i));
  }

  async function run() {
    if (!preview) return;
    busy = true;
    error = null;
    try {
      const profiles = preview.candidates
        .filter((_, i) => picked.has(i))
        .map(({ profile }) => {
          if (profile.protocol !== "sftp" || profile.auth.method !== "password" || sshAuth === "password") return profile;
          const auth: Auth =
            sshAuth === "agent" ? { method: "agent", key_fingerprint: null } : { method: "one_password", key_fingerprint: null };
          return { ...profile, auth };
        });
      const n = await api.importApply(profiles, workspace);
      onimported(workspace, n);
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }

  const authLabel = (a: Auth) => (a.method === "key_file" ? t("Key file") : t("Password"));
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

<div class="scrim" role="presentation" onclick={onclose}></div>
<div class="sheet" role="dialog" aria-modal="true" aria-labelledby="imp-title">
  <div class="sh">
    {#if preview}
      <button class="x back" onclick={() => ((preview = null), (error = null))} aria-label={t("Back")}><ArrowLeft size={16} /></button>
    {:else}
      <Import size={18} color="var(--pine)" />
    {/if}
    <h1 id="imp-title">{preview ? t("Import from {app}", { app: label }) : t("Import connections")}</h1>
    <button class="x" onclick={onclose} aria-label={t("Close")}><X size={16} /></button>
  </div>

  {#if !preview}
    <div class="apps" role="tablist">
      {#each APPS as a (a.id)}
        <button role="tab" class="app" class:on={source === a.id} aria-selected={source === a.id} onclick={() => ((source = a.id), (error = null))}>
          {a.label}
          {#if found.some((f) => f.source === a.id)}<span class="dot" title={t("Found on this computer")}></span>{/if}
        </button>
      {/each}
    </div>

    <p class="intro">
      {#if source === "cyberduck"}
        {t("Cyberduck keeps a .duck file per bookmark in its Bookmarks folder. Pick that folder, or a single .duck file.")}
      {:else if source === "filezilla"}
        {t("Kade reads FileZilla's Site Manager (sitemanager.xml), or a file made with File → Export.")}
      {:else}
        {t("In Transmit, choose Servers → Export… and leave “Include passwords” off, so the file isn't encrypted. Then pick that file here.")}
      {/if}
      {t("Passwords are never imported; Kade asks for them when you connect.")}
    </p>

    {#if here.length}
      <div class="lbl">{t("Found on this computer")}</div>
      {#each here as f (f.path)}
        <button class="found" disabled={busy} onclick={() => load(f.path)}>
          <FileSearch size={16} color="var(--pine)" />
          <span class="mono">{f.path}</span>
        </button>
      {/each}
    {/if}

    {#if error}<p class="err">{error}</p>{/if}

    <div class="actions">
      <span class="spacer"></span>
      {#if source === "cyberduck"}
        <button class="btn" disabled={busy} onclick={() => choose(true)}><FolderOpen size={14} />{t("Choose folder…")}</button>
      {/if}
      <button class="btn pri" disabled={busy} onclick={() => choose(false)}>{t("Choose file…")}</button>
    </div>
  {:else}
    <p class="path mono" title={chosenPath}>{chosenPath}</p>

    {#if preview.candidates.length}
      <div class="list">
        <label class="item head">
          <input type="checkbox" checked={picked.size === preview.candidates.length} indeterminate={picked.size > 0 && picked.size < preview.candidates.length} onchange={toggleAll} />
          <span>{tn(preview.candidates.length, "{n} connection", "{n} connections")}</span>
        </label>
        {#each preview.candidates as c, i (i)}
          <label class="item" class:dim={!picked.has(i)}>
            <input type="checkbox" checked={picked.has(i)} onchange={() => toggle(i)} />
            <span class="main">
              <b>{c.profile.name}</b>
              <small class="mono">{c.profile.protocol} · {c.profile.user ? `${c.profile.user}@` : ""}{c.profile.host}:{c.profile.port}</small>
            </span>
            {#if c.profile.group}<span class="tag">{c.profile.group}</span>{/if}
            {#if c.duplicate}<span class="tag warn">{t("Already in Kade")}</span>{:else}<span class="tag">{authLabel(c.profile.auth)}</span>{/if}
          </label>
        {/each}
      </div>
    {:else}
      <p class="intro">{t("No connections Kade can use in this file.")}</p>
    {/if}

    {#if preview.skipped.length}
      <details class="skipped">
        <summary><TriangleAlert size={13} />{tn(preview.skipped.length, "{n} bookmark skipped", "{n} bookmarks skipped")}</summary>
        <ul>{#each preview.skipped as s, i (i)}<li>{s}</li>{/each}</ul>
      </details>
    {/if}

    <div class="opts">
      <label class="f">
        <span>{t("Workspace")}</span>
        <select bind:value={workspace}>
          {#each workspaces as w (w.id)}<option value={w.id}>{w.name}</option>{/each}
        </select>
      </label>
      {#if sshWithoutKey}
        <label class="f">
          <span>{t("SFTP without a key file signs in with")}</span>
          <select bind:value={sshAuth}>
            <option value="password">{t("Password")}</option>
            <option value="one_password">{t("1Password SSH agent")}</option>
            <option value="agent">{t("SSH agent")}</option>
          </select>
        </label>
      {/if}
    </div>

    {#if error}<p class="err">{error}</p>{/if}

    <div class="actions">
      <span class="spacer"></span>
      <button class="btn ghost" onclick={onclose}>{t("Cancel")}</button>
      <button class="btn pri" disabled={busy || picked.size === 0} onclick={run}>
        {tn(picked.size, "Import {n} connection", "Import {n} connections")}
      </button>
    </div>
  {/if}
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    backdrop-filter: blur(3px);
    z-index: 20;
  }
  .sheet {
    position: fixed;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    width: min(560px, calc(100vw - 32px));
    max-height: calc(100vh - 32px);
    background: var(--paper);
    border-radius: 14px;
    box-shadow: var(--shadow-lg);
    z-index: 21;
    padding: 18px 20px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .sh {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  h1 {
    font-size: 16px;
    font-weight: 600;
  }
  .x {
    margin-left: auto;
    padding: 6px;
    border-radius: 6px;
    display: grid;
  }
  .x.back {
    margin-left: -6px;
  }
  .x:hover {
    background: var(--mist2);
  }
  .apps {
    display: flex;
    gap: 4px;
    padding: 3px;
    border-radius: 9px;
    background: var(--mist2);
  }
  .app {
    flex: 1;
    height: 32px;
    border-radius: 7px;
    font-weight: 500;
    color: var(--ink2);
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }
  .app.on {
    background: var(--paper);
    color: var(--granite);
    box-shadow: var(--shadow-sm);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--pine);
  }
  .intro {
    font-size: 12.5px;
    color: var(--ink2);
    line-height: 1.5;
  }
  .lbl {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--lichen);
  }
  .found {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--mist);
    border-radius: 9px;
    text-align: left;
    font-size: 12px;
    min-width: 0;
  }
  .found span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .found:hover:not(:disabled) {
    border-color: var(--pine);
    background: var(--pine-t);
  }
  .path {
    font-size: 11.5px;
    color: var(--lichen);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-top: -6px;
  }
  .list {
    border: 1px solid var(--mist);
    border-radius: 10px;
    overflow-y: auto;
    min-height: 0;
    max-height: 340px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border-top: 1px solid var(--mist2);
    cursor: pointer;
  }
  .item.head {
    border-top: 0;
    position: sticky;
    top: 0;
    background: var(--paper);
    font-weight: 600;
    font-size: 12.5px;
    border-bottom: 1px solid var(--mist);
  }
  .item.dim .main {
    opacity: 0.55;
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .main b,
  .main small {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .main b {
    font-weight: 500;
  }
  .main small {
    font-size: 11.5px;
    color: var(--lichen);
  }
  .tag {
    flex: none;
    font-size: 11px;
    padding: 2px 7px;
    border-radius: 6px;
    background: var(--mist2);
    color: var(--ink2);
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tag.warn {
    background: var(--amber-t);
    color: var(--amber-ink);
  }
  .skipped {
    font-size: 12px;
    color: var(--ink2);
  }
  .skipped summary {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
  }
  .skipped ul {
    margin: 6px 0 0 20px;
    color: var(--lichen);
  }
  .opts {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
  }
  .f {
    flex: 1;
    min-width: 200px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .f > span {
    font-weight: 500;
    font-size: 12.5px;
    color: var(--ink2);
  }
  select {
    height: 36px;
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 0 10px;
    background: var(--paper);
    outline: 0;
  }
  select:focus {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
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
</style>
