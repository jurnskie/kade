<script lang="ts">
  import { onMount, tick } from "svelte";
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
  import { listen } from "@tauri-apps/api/event";
  import { Check, CloudCheck, FolderSync, GitMerge, Import, X } from "@lucide/svelte";
  import { i18n, t, tn, type LangChoice } from "$lib/i18n.svelte";
  import { modalOpen } from "$lib/modals";
  import { PALETTES, theme, type ThemeChoice } from "$lib/theme.svelte";
  import { api, errorMessage, type EditorChoice, type McpStatus, type Settings, type SyncStatus, type UpdateInfo } from "$lib/api";

  let {
    settings,
    section = null,
    onchange,
    onimport,
    onclose,
  }: {
    settings: Settings;
    /** Section to scroll to on open, e.g. "about" for an update. */
    section?: string | null;
    onchange: () => void;
    onimport: () => void;
    onclose: () => void;
  } = $props();

  const sections = $derived([
    { id: "general", label: t("General") },
    { id: "files", label: t("Files") },
    { id: "sync", label: t("Sync") },
    { id: "backups", label: "Backups" },
    { id: "mcp", label: t("AI assistants (MCP)") },
    { id: "about", label: t("About Kade") },
  ]);

  let scroller: HTMLElement;
  let current = $state("general");

  /** Set while scrolling to a clicked section, which then stays highlighted. */
  let jumping = false;
  let jumpTimer: ReturnType<typeof setTimeout>;

  function jump(id: string) {
    current = id;
    jumping = true;
    document.getElementById(`set-${id}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
    // No scroll means no `scrollend`, e.g. when the section is already in place.
    clearTimeout(jumpTimer);
    jumpTimer = setTimeout(() => (jumping = false), 1000);
  }

  // Highlight the section being read: the last one whose heading has passed the top third.
  function track() {
    if (jumping) return;
    const edge = scroller.getBoundingClientRect().top + scroller.clientHeight / 3;
    let seen = sections[0].id;
    for (const s of sections) {
      const el = document.getElementById(`set-${s.id}`);
      if (el && el.getBoundingClientRect().top <= edge) seen = s.id;
    }
    // At the very bottom the last sections can never reach the edge.
    if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2) seen = sections.at(-1)!.id;
    current = seen;
  }

  onMount(async () => {
    if (!section) return;
    await tick();
    document.getElementById(`set-${section}`)?.scrollIntoView({ block: "start" });
  });

  let status = $state<SyncStatus | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  /** Run `fn`, showing a failure in the error note. */
  async function attempt(fn: () => Promise<unknown>) {
    try {
      await fn();
    } catch (e) {
      error = errorMessage(e);
    }
  }

  onMount(() => attempt(async () => (status = await api.syncStatus())));

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

  function toggleHidden() {
    return attempt(async () => {
      await api.saveSettings({ ...settings, show_hidden: !settings.show_hidden });
      onchange();
    });
  }

  let editorChoice = $state<EditorChoice | null>(null);
  /** "" = automatic, an option's command, or "custom". */
  let editorPick = $state("");
  let customEditor = $state("");
  onMount(() =>
    attempt(async () => {
      const c = await api.getEditor();
      editorChoice = c;
      const known = c.current === "" || c.options.some((o) => o.command === c.current);
      editorPick = known ? c.current : "custom";
      customEditor = known ? "" : c.current;
    }),
  );

  function pickEditor(pick: string) {
    editorPick = pick;
    return attempt(async () => {
      if (pick !== "custom") await api.setEditor(pick);
      else if (customEditor.trim()) await api.setEditor(customEditor.trim());
    });
  }

  function saveCustom() {
    return attempt(async () => {
      if (editorPick === "custom" && customEditor.trim()) await api.setEditor(customEditor.trim());
    });
  }

  let update = $state<UpdateInfo | null>(null);
  let checking = $state(false);
  let installing = $state<string | null>(null);

  async function checkUpdate() {
    checking = true;
    try {
      update = await api.updateCheck();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      checking = false;
    }
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
  onMount(() => attempt(async () => (mcp = await api.mcpStatus())));

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

  function copy(label: string, value: string) {
    return attempt(async () => {
      await navigator.clipboard.writeText(value);
      copied = label;
      setTimeout(() => (copied = null), 1500);
    });
  }

  function setRetention(e: Event) {
    const days = Number((e.target as HTMLSelectElement).value);
    return attempt(async () => {
      await api.saveSettings({ ...settings, backup_retention_days: days });
      onchange();
    });
  }

  const themes: [ThemeChoice, string][] = $derived([
    ["auto", t("Automatic")],
    ["light", t("Light")],
    ["dark", t("Dark")],
  ]);
  const langs: [LangChoice, string][] = $derived([
    ["auto", t("Automatic")],
    ["en", "English"],
    ["nl", "Nederlands"],
  ]);
</script>

<!-- Escape closes the page, unless a dialog on top of it takes it. -->
<svelte:window
  onkeydown={(e) => e.key === "Escape" && !e.defaultPrevented && !modalOpen() && onclose()}
/>

{#snippet segmented(name: string, label: string, options: [string, string][], value: string, set: (v: string) => void)}
  <div class="seg" role="radiogroup" aria-label={label}>
    {#each options as [v, text] (v)}
      <label class:on={value === v}>
        <input type="radio" {name} checked={value === v} onchange={() => set(v)} />{text}
      </label>
    {/each}
  </div>
{/snippet}

{#snippet toggle(label: string, on: boolean, flip: () => void)}
  <button class="switch" class:on role="switch" aria-checked={on} aria-label={label} onclick={flip}><i></i></button>
{/snippet}

<div class="page" role="region" aria-labelledby="set-title">
  <header>
    <h1 id="set-title">{t("Settings")}</h1>
    <button class="btn close" onclick={onclose} title={t("Close")} aria-label={t("Close")}><X size={16} /></button>
  </header>

  <div class="body" bind:this={scroller} onscroll={track} onscrollend={() => (clearTimeout(jumpTimer), (jumping = false))}>
    <nav aria-label={t("Settings")}>
      {#each sections as s (s.id)}
        <button class:on={current === s.id} aria-current={current === s.id ? "true" : undefined} onclick={() => jump(s.id)}>
          {s.label}
        </button>
      {/each}
    </nav>

    <div class="content">
      {#if error}<div class="note err" role="alert">{error}</div>{/if}

      <section id="set-general">
        <h2>{t("General")}</h2>
        <div class="group">
          <div class="row">
            <div class="what"><b>{t("Mode")}</b><small>{t("Automatic follows your system.")}</small></div>
            {@render segmented("theme", t("Mode"), themes, theme.choice, (v) => theme.set(v as ThemeChoice))}
          </div>
          <div class="row palette">
            <div class="what"><b>{t("Colour theme")}</b><small>{t("Each theme has a light and a dark version.")}</small></div>
            <div class="cards" role="radiogroup" aria-label={t("Colour theme")}>
              {#each PALETTES as p (p.id)}
                {@const on = theme.palette === p.id}
                <label class="card" class:on>
                  <input type="radio" name="palette" checked={on} onchange={() => theme.setPalette(p.id)} />
                  <!-- Painted by the palette's own tokens, in the scheme that's active now. -->
                  <span class="mini" data-palette={p.id} data-scheme={theme.scheme} aria-hidden="true">
                    <span class="m-rail"><i></i><i></i><i></i></span>
                    <span class="m-pane">
                      <i class="bar"></i><i class="bar short"></i>
                      <i class="pill"></i>
                      <i class="sel"></i>
                    </span>
                  </span>
                  <span class="name">{p.name}{#if on}<Check size={13} />{/if}</span>
                </label>
              {/each}
            </div>
          </div>
          <div class="row">
            <div class="what"><b>{t("Language of the app")}</b></div>
            {@render segmented("lang", t("Language"), langs, i18n.choice, (v) => i18n.set(v as LangChoice))}
          </div>
        </div>
      </section>

      <section id="set-files">
        <h2>{t("Files")}</h2>
        <div class="group">
          <div class="row">
            <div class="what">
              <b>{t("Show hidden files by default")}</b>
              <small>{t("Dotfiles such as .env and .htaccess. Each pane can still switch on its own.")}</small>
            </div>
            {@render toggle(t("Show hidden files by default"), settings.show_hidden, toggleHidden)}
          </div>
        </div>

        <h3>Editor</h3>
        <p class="intro">
          {t("What files open in when you double-click them. Server files are uploaded every time you save, with a backup of the previous version. This choice applies to this computer only.")}
        </p>
        <div class="group">
          <label class="row pick" class:on={editorPick === ""}>
            <input type="radio" name="editor" checked={editorPick === ""} onchange={() => pickEditor("")} />
            <div class="what"><b>{t("Automatic")}</b><small>{editorChoice?.automatic ?? "…"}</small></div>
          </label>
          {#each editorChoice?.options ?? [] as o (o.command)}
            <label class="row pick" class:on={editorPick === o.command}>
              <input type="radio" name="editor" checked={editorPick === o.command} onchange={() => pickEditor(o.command)} />
              <div class="what">
                <b>{o.label}</b>
                <small class="mono">{o.command}{o.terminal ? ` · ${t("in terminal")}` : ""}</small>
              </div>
            </label>
          {/each}
          <label class="row pick" class:on={editorPick === "custom"}>
            <input type="radio" name="editor" checked={editorPick === "custom"} onchange={() => pickEditor("custom")} />
            <div class="what">
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
            </div>
          </label>
        </div>
      </section>

      <section id="set-sync">
        <h2>{t("Sync")}</h2>
        <p class="intro">
          {t("Choose a folder your sync client keeps in step (Synology Drive, Syncthing, iCloud…). Kade stores all connections and settings there in kade.json. Passwords are never stored.")}
        </p>
        <div class="group">
          <div class="row">
            {#if status?.sync_dir}
              <CloudCheck size={18} color="var(--pine)" />
              <div class="what"><b>{t("Synced")}</b><small class="mono path">{status.data_file}</small></div>
            {:else}
              <FolderSync size={18} color="var(--lichen)" />
              <div class="what"><b>{t("Only on this computer")}</b><small class="mono path">{status?.data_file ?? "…"}</small></div>
            {/if}
          </div>
          {#if status?.merged_conflicts.length}
            <div class="row note">
              <GitMerge size={14} />{tn(status.merged_conflicts.length, "Merged {n} conflict copy: {files}", "Merged {n} conflict copies: {files}", { files: status.merged_conflicts.join(", ") })}
            </div>
          {/if}
          <div class="row actions">
            <button class="btn pri" disabled={busy} onclick={choose}>
              {status?.sync_dir ? t("Choose another folder…") : t("Choose folder…")}
            </button>
            {#if status?.sync_dir}
              <button class="btn" disabled={busy} onclick={() => setDir(null)}>{t("Stop (keep a local copy)")}</button>
            {/if}
          </div>
        </div>

        <h3>{t("Import")}</h3>
        <div class="group">
          <div class="row">
            <div class="what">
              <small>{t("Bring your connections over from ~/.ssh/config, Cyberduck, FileZilla or Transmit. Passwords stay behind.")}</small>
            </div>
            <button class="btn" onclick={onimport}><Import size={14} />{t("Import connections…")}</button>
          </div>
        </div>
      </section>

      <section id="set-backups">
        <h2>Backups</h2>
        <div class="group">
          <div class="row">
            <div class="what">
              <b>{t("Keep backups of deleted and overwritten files")}</b>
              <small>
                {t("On servers they're kept in ~/.cache/kade/backups, on this computer in Kade's data folder. Old backups on a server are cleaned up the next time you connect to it.")}
              </small>
            </div>
            <select value={settings.backup_retention_days} onchange={setRetention} aria-label={t("Keep backups of deleted and overwritten files")}>
              {#each [1, 3, 7, 14, 30] as d (d)}<option value={d}>{tn(d, "{n} day", "{n} days")}</option>{/each}
            </select>
          </div>
        </div>
      </section>

      <section id="set-mcp">
        <h2>{t("AI assistants (MCP)")}</h2>
        <div class="group">
          <div class="row">
            <div class="what">
              <b>{t("MCP server")}</b>
              <small>{t("Turn on the MCP server, so e.g. Claude Code can manage groups and connections")}</small>
            </div>
            {@render toggle(t("MCP server"), mcp?.enabled ?? false, toggleMcp)}
          </div>
          {#if mcp?.enabled}
            {#if mcp.error}
              <div class="row note err">{mcp.error}</div>
            {:else if mcp.running}
              <div class="row">
                <div class="what">
                  <b>{t("Running on")} <span class="mono">{mcp.url}</span></b>
                  <small>{t("Only reachable from this computer, and only with the token.")}</small>
                </div>
              </div>
              <div class="row kv">
                <span>Token</span>
                <code class="mono">{showToken ? mcp.token : "•".repeat(24)}</code>
                <button class="btn sm" onclick={() => (showToken = !showToken)}>{showToken ? t("Hide") : t("Show")}</button>
                <button class="btn sm" onclick={() => copy("token", mcp!.token)}>{copied === "token" ? t("Copied") : t("Copy")}</button>
              </div>
              <div class="row stack">
                <small>{t("Add to Claude Code (once, in a terminal):")}</small>
                <div class="cmd">
                  <code class="mono">{showToken ? claudeCommand : claudeCommand.replace(mcp.token, "•••")}</code>
                  <button class="btn sm" onclick={() => copy("cmd", claudeCommand)}>{copied === "cmd" ? t("Copied") : t("Copy")}</button>
                </div>
              </div>
              <div class="row">
                <button class="btn sm ghost" onclick={() => attempt(async () => (mcp = await api.mcpRegenerateToken()))}>
                  {t("Create a new token (the old one stops working)")}
                </button>
              </div>
            {/if}
          {/if}
        </div>
      </section>

      <section id="set-about">
        <h2>{t("About Kade")}</h2>
        <div class="group">
          <div class="row" class:fresh={update?.newer}>
            <div class="what">
              <b>Kade {update?.current ?? ""}</b>
              <small>
                {#if checking}{t("Checking for updates…")}
                {:else if update?.error}{update.error}
                {:else if update?.newer}{t("Version {v} is available", { v: update.latest ?? "" })}
                {:else if update?.latest}{t("You have the latest version")}
                {/if}
              </small>
              {#if update?.newer && update.install.kind === "other"}<small>{update.install.reason}</small>{/if}
            </div>
            {#if update?.newer && update.install.kind !== "other"}
              <button class="btn pri" disabled={installing != null} onclick={installUpdate}>
                {installing ?? t("Update to {v} and restart", { v: update.latest ?? "" })}
              </button>
            {:else}
              <button class="btn" disabled={checking} onclick={checkUpdate}>{t("Check for updates")}</button>
            {/if}
          </div>
        </div>
      </section>
    </div>
  </div>
</div>

<style>
  .page {
    container: settings / inline-size;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--snow);
  }
  header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 22px 28px 14px;
    border-bottom: 1px solid var(--mist);
  }
  h1 {
    font-size: 22px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .close {
    margin-left: auto;
    padding: 6px;
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: grid;
    grid-template-columns: 180px minmax(0, 660px);
    gap: 40px;
    padding: 24px 28px 64px;
  }
  nav {
    position: sticky;
    top: 0;
    align-self: start;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  nav button {
    text-align: left;
    padding: 6px 10px;
    border-radius: 7px;
    color: var(--ink2);
    border-left: 2px solid transparent;
  }
  nav button:hover {
    background: var(--mist2);
  }
  nav button.on {
    color: var(--granite);
    font-weight: 600;
    background: var(--pine-t);
    border-left-color: var(--pine);
    border-radius: 0 7px 7px 0;
  }
  .content {
    display: flex;
    flex-direction: column;
    gap: 36px;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
    scroll-margin-top: 4px;
  }
  h2 {
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  h3 {
    font-size: 13px;
    font-weight: 600;
    margin-top: 14px;
  }
  .intro {
    color: var(--ink2);
    line-height: 1.5;
    font-size: 12.5px;
    max-width: 62ch;
  }
  .group {
    display: flex;
    flex-direction: column;
    background: var(--paper);
    border: 1px solid var(--mist);
    border-radius: 10px;
    overflow: hidden;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 12px 14px;
    min-height: 52px;
  }
  .row + .row {
    border-top: 1px solid var(--mist2);
  }
  .what {
    flex: 1;
    min-width: 0;
  }
  .what b {
    display: block;
    font-weight: 600;
  }
  .what small,
  .stack small {
    display: block;
    margin-top: 2px;
    font-size: 12px;
    line-height: 1.45;
    color: var(--lichen);
  }
  .path {
    word-break: break-all;
  }
  .fresh {
    background: var(--pine-t);
  }

  /* Editor choice: a whole row is the radio. */
  .pick {
    align-items: flex-start;
    cursor: pointer;
  }
  .pick input[type="radio"] {
    accent-color: var(--pine);
    margin-top: 3px;
  }
  .pick.on {
    background: var(--pine-t);
  }
  .custom {
    width: 100%;
    margin: 6px 0 2px;
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

  .seg {
    display: inline-flex;
    flex-shrink: 0;
    padding: 2px;
    border-radius: 8px;
    background: var(--mist2);
    border: 1px solid var(--mist);
  }
  .seg label {
    position: relative;
    padding: 4px 11px;
    border-radius: 6px;
    font-size: 12.5px;
    color: var(--ink2);
    cursor: pointer;
  }
  .seg label.on {
    background: var(--paper);
    color: var(--granite);
    font-weight: 600;
    box-shadow: var(--shadow-sm);
  }
  .seg input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .seg label:has(input:focus-visible) {
    outline: 2px solid var(--pine);
    outline-offset: 1px;
  }

  /* Colour theme: the cards sit below the label instead of in the segmented slot. */
  .row.palette {
    flex-direction: column;
    align-items: stretch;
    gap: 10px;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(132px, 1fr));
    gap: 10px;
  }
  .card {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding: 6px 6px 8px;
    border: 2px solid transparent;
    border-radius: 10px;
    cursor: pointer;
  }
  .card:hover {
    background: var(--mist2);
  }
  .card.on {
    border-color: var(--pine);
  }
  .card input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .card:has(input:focus-visible) {
    outline: 2px solid var(--pine);
    outline-offset: 1px;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 2px;
    font-weight: 600;
    color: var(--granite);
  }
  .mini {
    display: flex;
    height: 68px;
    border-radius: 6px;
    overflow: hidden;
    background: var(--snow);
    border: 1px solid var(--mist);
  }
  .m-rail {
    flex: none;
    width: 28%;
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 8px 6px;
    background: var(--rail);
    border-right: 1px solid var(--mist);
  }
  .m-rail i {
    height: 3px;
    border-radius: 2px;
    background: var(--mist);
  }
  .m-pane {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin: 6px;
    padding: 6px;
    border-radius: 4px;
    background: var(--paper);
  }
  .m-pane i {
    display: block;
    height: 4px;
    border-radius: 2px;
  }
  .m-pane .bar {
    background: var(--ink2);
  }
  .m-pane .short {
    width: 60%;
  }
  .m-pane .pill {
    width: 36%;
    height: 8px;
    background: var(--accent);
    border-radius: 4px;
  }
  .m-pane .sel {
    height: 8px;
    margin-top: auto;
    border-radius: 3px;
    background: var(--accent-t);
  }

  .switch {
    flex-shrink: 0;
    width: 36px;
    height: 20px;
    border-radius: 999px;
    background: var(--control);
    position: relative;
    transition: background 0.15s;
  }
  .switch i {
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
  .switch.on {
    background: var(--pine);
  }
  .switch.on i {
    transform: translateX(16px);
  }
  .switch:focus-visible {
    outline: 2px solid var(--pine);
    outline-offset: 2px;
  }

  select {
    flex-shrink: 0;
    border: 1px solid var(--mist);
    border-radius: 6px;
    padding: 4px 6px;
    background: var(--paper);
  }
  .note {
    gap: 8px;
    font-size: 12px;
    color: var(--ink2);
    min-height: 0;
  }
  .err {
    color: var(--danger);
  }
  .content > .note.err {
    padding: 10px 12px;
    border: 1px solid var(--danger-line);
    background: var(--danger-t);
    border-radius: 8px;
  }
  .actions {
    flex-wrap: wrap;
    gap: 8px;
  }
  .kv {
    gap: 8px;
    font-size: 12.5px;
  }
  .kv code,
  .cmd code {
    flex: 1;
    min-width: 0;
    font-size: 11.5px;
    background: var(--snow);
    border: 1px solid var(--mist);
    border-radius: 6px;
    padding: 4px 8px;
  }
  .kv code {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .stack {
    flex-direction: column;
    align-items: stretch;
    gap: 6px;
  }
  .cmd {
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }
  .cmd code {
    /* Wrap at spaces; a long token (bearer token, URL) scrolls instead of splitting mid-word. */
    white-space: pre-wrap;
    word-break: normal;
    overflow-wrap: normal;
    overflow-x: auto;
    padding: 6px 8px;
  }
  .btn.sm {
    padding: 4px 9px;
    font-size: 12px;
  }

  /* Narrow windows: the index moves above the content as a row of chips. */
  @container settings (max-width: 700px) {
    .body {
      grid-template-columns: minmax(0, 1fr);
      gap: 16px;
      padding: 16px 16px 48px;
    }
    nav {
      flex-direction: row;
      flex-wrap: wrap;
      gap: 4px;
      position: static;
    }
    nav button,
    nav button.on {
      border-left: 0;
      border-radius: 7px;
    }
    header {
      padding: 16px;
    }
    .row {
      flex-wrap: wrap;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .switch,
    .switch i {
      transition: none;
    }
  }
</style>
