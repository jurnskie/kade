<script lang="ts">
  import { onMount } from "svelte";
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
  import { Settings as Gear, X, FolderSync, CloudCheck, GitMerge, Import } from "@lucide/svelte";
  import { listen } from "@tauri-apps/api/event";
  import { i18n, t, tn, type LangChoice } from "$lib/i18n.svelte";
  import { theme, type ThemeChoice } from "$lib/theme.svelte";
  import { api, errorMessage, type EditorChoice, type McpStatus, type Settings, type SyncStatus, type UpdateInfo } from "$lib/api";

  let {
    settings,
    onchange,
    onimport,
    onclose,
  }: { settings: Settings; onchange: () => void; onimport: () => void; onclose: () => void } = $props();

  let status = $state<SyncStatus | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  onMount(async () => (status = await api.syncStatus()));

  async function setDir(dir: string | null) {
    busy = true;
    error = null;
    try {
      status = await api.setSyncDir(dir);
      onchange();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }

  async function choose() {
    const dir = await pickFolder({ directory: true, title: t("Choose a sync folder") });
    if (typeof dir === "string") await setDir(dir);
  }

  async function toggleHidden() {
    await api.saveSettings({ ...settings, show_hidden: !settings.show_hidden });
    onchange();
  }

  let editorChoice = $state<EditorChoice | null>(null);
  /** "" = automatic, an option's command, or "custom". */
  let editorPick = $state("");
  let customEditor = $state("");
  onMount(async () => {
    const c = await api.getEditor();
    editorChoice = c;
    const known = c.current === "" || c.options.some((o) => o.command === c.current);
    editorPick = known ? c.current : "custom";
    customEditor = known ? "" : c.current;
  });

  async function pickEditor(pick: string) {
    editorPick = pick;
    if (pick !== "custom") await api.setEditor(pick);
    else if (customEditor.trim()) await api.setEditor(customEditor.trim());
  }

  async function saveCustom() {
    if (editorPick === "custom" && customEditor.trim()) await api.setEditor(customEditor.trim());
  }

  let update = $state<UpdateInfo | null>(null);
  let checking = $state(false);
  let installing = $state<string | null>(null);

  async function checkUpdate() {
    checking = true;
    update = await api.updateCheck();
    checking = false;
  }
  onMount(() => {
    checkUpdate();
    const off = listen<string>("update-progress", ({ payload }) => (installing = payload));
    return () => off.then((f) => f());
  });

  async function installUpdate() {
    if (!update?.latest) return;
    installing = t("Starting…");
    try {
      await api.updateInstall(update.latest);
    } catch (e) {
      installing = null;
      error = errorMessage(e);
    }
  }

  let mcp = $state<McpStatus | null>(null);
  let showToken = $state(false);
  let copied = $state<string | null>(null);
  onMount(async () => (mcp = await api.mcpStatus()));

  const claudeCommand = $derived(
    mcp ? `claude mcp add --transport http kade ${mcp.url} --header "Authorization: Bearer ${mcp.token}"` : "",
  );

  async function toggleMcp() {
    try {
      mcp = await api.mcpSetEnabled(!mcp?.enabled);
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function copy(label: string, value: string) {
    await navigator.clipboard.writeText(value);
    copied = label;
    setTimeout(() => (copied = null), 1500);
  }

  async function setRetention(e: Event) {
    const days = Number((e.target as HTMLSelectElement).value);
    await api.saveSettings({ ...settings, backup_retention_days: days });
    onchange();
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

<div class="scrim" role="presentation" onclick={onclose}></div>
<div class="sheet" role="dialog" aria-modal="true">
  <div class="sh">
    <Gear size={18} color="var(--pine)" />
    <h1>{t("Settings")}</h1>
    <button class="x" onclick={onclose} aria-label={t("Close")}><X size={16} /></button>
  </div>

  <section>
    <div class="lbl">{t("About Kade")}</div>
    <div class="card">
      <div class="t">
        <b>Kade {update?.current ?? ""}</b>
        <small>
          {#if checking}{t("Checking for updates…")}
          {:else if update?.error}{update.error}
          {:else if update?.newer}{t("Version {v} is available", { v: update.latest ?? "" })}
          {:else if update?.latest}{t("You have the latest version")}
          {/if}
        </small>
      </div>
    </div>
    <div class="actions">
      {#if update?.newer && update.install.kind !== "other"}
        <button class="btn pri" disabled={installing != null} onclick={installUpdate}>
          {installing ?? t("Update to {v} and restart", { v: update.latest ?? "" })}
        </button>
      {:else}
        <button class="btn" disabled={checking} onclick={checkUpdate}>{t("Check for updates")}</button>
      {/if}
    </div>
    {#if update?.newer && update.install.kind === "other"}
      <p class="intro small">{update.install.reason}</p>
    {/if}
  </section>

  <section>
    <div class="lbl">{t("Appearance")}</div>
    <label class="row">
      {t("Theme")}
      <select value={theme.choice} onchange={(e) => theme.set((e.target as HTMLSelectElement).value as ThemeChoice)}>
        <option value="auto">{t("Automatic (system)")}</option>
        <option value="light">{t("Light")}</option>
        <option value="dark">{t("Dark")}</option>
      </select>
    </label>
  </section>

  <section>
    <div class="lbl">{t("Language")}</div>
    <label class="row">
      {t("Language of the app")}
      <select value={i18n.choice} onchange={(e) => i18n.set((e.target as HTMLSelectElement).value as LangChoice)}>
        <option value="auto">{t("Automatic (system)")}</option>
        <option value="en">English</option>
        <option value="nl">Nederlands</option>
      </select>
    </label>
  </section>

  <section>
    <div class="lbl">{t("Sync")}</div>
    <p class="intro">
      {t("Choose a folder your sync client keeps in step (Synology Drive, Syncthing, iCloud…). Kade stores all connections and settings there in kade.json. Passwords are never stored.")}
    </p>
    <div class="card">
      {#if status?.sync_dir}
        <CloudCheck size={18} color="var(--pine)" />
        <div class="t"><b>{t("Synced")}</b><small class="mono">{status.data_file}</small></div>
      {:else}
        <FolderSync size={18} color="var(--ink2)" />
        <div class="t"><b>{t("Only on this computer")}</b><small class="mono">{status?.data_file ?? "…"}</small></div>
      {/if}
    </div>
    {#if status?.merged_conflicts.length}
      <div class="note">
        <GitMerge size={14} />{tn(status.merged_conflicts.length, "Merged {n} conflict copy: {files}", "Merged {n} conflict copies: {files}", { files: status.merged_conflicts.join(", ") })}
      </div>
    {/if}
    {#if error}<div class="note err">{error}</div>{/if}
    <div class="actions">
      <button class="btn pri" disabled={busy} onclick={choose}>
        {status?.sync_dir ? t("Choose another folder…") : t("Choose folder…")}
      </button>
      {#if status?.sync_dir}
        <button class="btn" disabled={busy} onclick={() => setDir(null)}>{t("Stop (keep a local copy)")}</button>
      {/if}
    </div>
  </section>

  <section>
    <div class="lbl">{t("Import")}</div>
    <p class="intro small">{t("Bring your connections over from ~/.ssh/config, Cyberduck, FileZilla or Transmit. Passwords stay behind.")}</p>
    <div class="actions">
      <button class="btn" onclick={onimport}><Import size={14} />{t("Import connections…")}</button>
    </div>
  </section>

  <section>
    <div class="lbl">{t("Files")}</div>
    <label class="row">
      <input type="checkbox" checked={settings.show_hidden} onchange={toggleHidden} />
      {t("Show hidden files by default")}
    </label>
  </section>

  <section>
    <div class="lbl">Editor</div>
    <p class="intro small">
      {t("What files open in when you double-click them. Server files are uploaded every time you save, with a backup of the previous version. This choice applies to this computer only.")}
    </p>
    <div class="choices">
      <label class="choice" class:on={editorPick === ""}>
        <input type="radio" name="editor" checked={editorPick === ""} onchange={() => pickEditor("")} />
        <span><b>{t("Automatic")}</b><small>{editorChoice?.automatic ?? "…"}</small></span>
      </label>
      {#each editorChoice?.options ?? [] as o (o.command)}
        <label class="choice" class:on={editorPick === o.command}>
          <input type="radio" name="editor" checked={editorPick === o.command} onchange={() => pickEditor(o.command)} />
          <span>
            <b>{o.label}</b>
            <small class="mono">{o.command}{o.terminal ? ` · ${t("in terminal")}` : ""}</small>
          </span>
        </label>
      {/each}
      <label class="choice" class:on={editorPick === "custom"}>
        <input type="radio" name="editor" checked={editorPick === "custom"} onchange={() => pickEditor("custom")} />
        <span class="grow">
          <b>{t("Custom command")}</b>
          {#if editorPick === "custom"}
            <input
              class="mono custom"
              bind:value={customEditor}
              placeholder={t("e.g. code --new-window or ghostty -e hx")}
              spellcheck="false"
              onblur={saveCustom}
              onkeydown={(e) => e.key === "Enter" && saveCustom()}
            />
            <small>{t("The path goes at the end, or wherever {file} appears.")}</small>
          {:else}
            <small>{t("Any program that takes a file path")}</small>
          {/if}
        </span>
      </label>
    </div>
  </section>

  <section>
    <div class="lbl">{t("AI assistants (MCP)")}</div>
    <label class="row">
      <input type="checkbox" checked={mcp?.enabled ?? false} onchange={toggleMcp} />
      {t("Turn on the MCP server, so e.g. Claude Code can manage groups and connections")}
    </label>
    {#if mcp?.enabled}
      {#if mcp.error}
        <div class="note err">{mcp.error}</div>
      {:else if mcp.running}
        <div class="card">
          <div class="t">
            <b>{t("Running on")} <span class="mono">{mcp.url}</span></b>
            <small>{t("Only reachable from this computer, and only with the token.")}</small>
          </div>
        </div>
        <div class="kv">
          <span>Token</span>
          <code class="mono">{showToken ? mcp.token : "•".repeat(24)}</code>
          <button class="btn sm" onclick={() => (showToken = !showToken)}>{showToken ? t("Hide") : t("Show")}</button>
          <button class="btn sm" onclick={() => copy("token", mcp!.token)}>{copied === "token" ? t("Copied") : t("Copy")}</button>
        </div>
        <p class="intro small">{t("Add to Claude Code (once, in a terminal):")}</p>
        <div class="cmd">
          <code class="mono">{showToken ? claudeCommand : claudeCommand.replace(mcp.token, "•••")}</code>
          <button class="btn sm" onclick={() => copy("cmd", claudeCommand)}>{copied === "cmd" ? t("Copied") : t("Copy")}</button>
        </div>
        <button class="btn sm ghost" onclick={async () => (mcp = await api.mcpRegenerateToken())}>
          {t("Create a new token (the old one stops working)")}
        </button>
      {/if}
    {/if}
  </section>

  <section>
    <div class="lbl">Backups</div>
    <label class="row">
      {t("Keep backups of deleted and overwritten files")}
      <select value={settings.backup_retention_days} onchange={setRetention}>
        {#each [1, 3, 7, 14, 30] as d (d)}<option value={d}>{tn(d, "{n} day", "{n} days")}</option>{/each}
      </select>
    </label>
    <p class="intro small">
      {t("On servers they're kept in ~/.cache/kade/backups, on this computer in Kade's data folder. Old backups on a server are cleaned up the next time you connect to it.")}
    </p>
  </section>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    backdrop-filter: blur(3px);
    z-index: 10;
  }
  .sheet {
    position: fixed;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    width: min(560px, calc(100vw - 32px));
    max-height: calc(100vh - 32px);
    overflow-y: auto;
    background: var(--paper);
    border-radius: 16px;
    box-shadow: var(--shadow-lg);
    z-index: 11;
    padding: 20px 22px 22px;
    display: flex;
    flex-direction: column;
    gap: 20px;
    user-select: text;
  }
  .sh {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  h1 {
    font-size: 18px;
    font-weight: 600;
  }
  .x {
    margin-left: auto;
    padding: 6px;
    border-radius: 6px;
  }
  .x:hover {
    background: var(--mist2);
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .intro {
    color: var(--ink2);
    line-height: 1.5;
  }
  .card {
    display: flex;
    align-items: center;
    gap: 12px;
    border: 1px solid var(--mist);
    border-radius: 10px;
    background: var(--snow);
    padding: 10px 12px;
  }
  .t {
    min-width: 0;
  }
  .t b {
    display: block;
    font-weight: 600;
  }
  .t small {
    display: block;
    font-size: 11.5px;
    color: var(--lichen);
    word-break: break-all;
  }
  .note {
    display: flex;
    gap: 8px;
    align-items: center;
    font-size: 12px;
    color: var(--ink2);
  }
  .note.err {
    color: var(--danger);
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .row input {
    accent-color: var(--pine);
  }
  .kv {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
  }
  .kv code {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 12px;
    background: var(--snow);
    border: 1px solid var(--mist);
    border-radius: 6px;
    padding: 4px 8px;
  }
  .cmd {
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }
  .cmd code {
    flex: 1;
    min-width: 0;
    font-size: 11.5px;
    word-break: break-all;
    background: var(--snow);
    border: 1px solid var(--mist);
    border-radius: 6px;
    padding: 6px 8px;
  }
  .btn.sm {
    padding: 4px 9px;
    font-size: 12px;
  }
  .choices {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--mist);
    border-radius: 10px;
    overflow: hidden;
  }
  .choice {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 9px 12px;
    cursor: pointer;
  }
  .choice + .choice {
    border-top: 1px solid var(--mist2);
  }
  .choice.on {
    background: var(--pine-t);
  }
  .choice input[type="radio"] {
    accent-color: var(--pine);
    margin-top: 3px;
  }
  .choice b {
    display: block;
    font-weight: 600;
  }
  .choice small {
    display: block;
    font-size: 11.5px;
    color: var(--lichen);
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .custom {
    width: 100%;
    margin: 6px 0 4px;
    height: 32px;
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 0 10px;
    font-size: 12.5px;
    outline: 0;
    background: var(--paper);
  }
  .custom:focus {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
  }
  .row select {
    margin-left: auto;
    border: 1px solid var(--mist);
    border-radius: 6px;
    padding: 4px 6px;
    background: var(--paper);
  }
  .small {
    font-size: 12px;
    color: var(--lichen);
  }
</style>
