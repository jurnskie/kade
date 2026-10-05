<script lang="ts">
  import { onMount } from "svelte";
  import {
    Server,
    SquareTerminal,
    FolderUp,
    Columns2,
    Activity,
    Laptop,
    X,
    Plus,
    Unplug,
    TriangleAlert,
    Fingerprint,
    LoaderCircle,
    Anchor,
    Undo2,
  } from "@lucide/svelte";
  import { api, errorMessage, localOps, remoteOps, type AppError, type ServerProfile } from "$lib/api";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import FilePane from "$lib/components/FilePane.svelte";
  import ConnectDialog from "$lib/components/ConnectDialog.svelte";
  import OnePasswordIcon from "$lib/components/OnePasswordIcon.svelte";
  import TerminalView from "$lib/components/TerminalView.svelte";
  import SettingsDialog from "$lib/components/SettingsDialog.svelte";
  import BackupsDialog from "$lib/components/BackupsDialog.svelte";
  import QuickSwitcher from "$lib/components/QuickSwitcher.svelte";
  import WorkspaceDialog from "$lib/components/WorkspaceDialog.svelte";
  import { colorOf, workspaceIdOf } from "$lib/workspaces";
  import { i18n, syncBackendLanguage, t } from "$lib/i18n.svelte";
  import TransferQueue from "$lib/components/TransferQueue.svelte";
  import EditsBar from "$lib/components/EditsBar.svelte";
  import { edits } from "$lib/edits.svelte";
  import { transfers } from "$lib/transfers.svelte";
  import { drag, targetAt } from "$lib/drag.svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { listen } from "@tauri-apps/api/event";
  import type { ConflictPolicy, Direction, Entry, Settings, Transaction, Workspace } from "$lib/api";

  interface Tab {
    sessionId: string;
    server: ServerProfile;
    authLabel: string;
    remotePath: string;
    localPath: string;
    hasFiles: boolean;
    hasTerminal: boolean;
    view: "files" | "terminal";
    /** Mount the terminal lazily, then keep it alive across view/tab switches. */
    terminalStarted: boolean;
    localRefresh: number;
    remoteRefresh: number;
  }

  type Prompt =
    | { kind: "hostkey"; server: ServerProfile; fingerprint: string; algorithm: string; password?: string }
    | { kind: "password"; server: ServerProfile; acceptFingerprint?: string };

  let servers = $state<ServerProfile[]>([]);
  let tabs = $state<Tab[]>([]);
  let activeId = $state<string | null>(null);
  let localHome = $state("");
  let dialog = $state<{ initial: ServerProfile | null; key: number } | null>(null);
  let prompt = $state<Prompt | null>(null);
  let passwordInput = $state("");
  let connecting = $state<string | null>(null);
  let toast = $state<string | null>(null);
  let settings = $state<Settings>({ show_hidden: false, backup_retention_days: 7, updated_at: 0 });
  let settingsOpen = $state(false);
  let backupsOpen = $state(false);
  let conflictAsk = $state<{ names: string[]; dest: string; resolve: (p: ConflictPolicy | null) => void } | null>(null);
  let undo = $state<{ tx: Transaction; sessionId: string | null } | null>(null);
  let updateAvailable = $state<string | null>(null);
  let switcherOpen = $state(false);

  // Workspaces are synced; which one is active is remembered per machine.
  const WS_KEY = "kade.workspace";
  let workspaces = $state<Workspace[]>([]);
  let activeWsId = $state<string>(
    (() => {
      try {
        return localStorage.getItem(WS_KEY) ?? "";
      } catch {
        return "";
      }
    })(),
  );
  let wsDialog = $state<{ initial: Workspace | null; key: number } | null>(null);

  function switchWorkspace(id: string) {
    activeWsId = id;
    try {
      localStorage.setItem(WS_KEY, id);
    } catch {
      /* not persisted; fine */
    }
  }

  // Recently opened connections, per machine (a convenience; storage may fail).
  const RECENT_KEY = "kade.recent";
  let recent = $state<string[]>(
    (() => {
      try {
        return JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
      } catch {
        return [];
      }
    })(),
  );
  function remember(id: string) {
    recent = [id, ...recent.filter((r) => r !== id)].slice(0, 10);
    try {
      localStorage.setItem(RECENT_KEY, JSON.stringify(recent));
    } catch {
      /* not persisted; fine */
    }
  }
  let undoTimer: ReturnType<typeof setTimeout> | undefined;

  async function reloadStore() {
    workspaces = await api.listWorkspaces();
    if (!workspaces.some((w) => w.id === activeWsId)) activeWsId = workspaces[0]?.id ?? "";
    servers = await api.listServers();
    settings = await api.getSettings();
  }

  // Fold the sidebar into an icon rail on narrow windows (half-screen tiling);
  // only react when crossing the threshold so a manual toggle sticks.
  const NARROW = 1000;
  let innerWidth = $state(window.innerWidth);
  let collapsed = $state(window.innerWidth < NARROW);
  let wasNarrow = window.innerWidth < NARROW;
  $effect(() => {
    const narrow = innerWidth < NARROW;
    if (narrow !== wasNarrow) {
      wasNarrow = narrow;
      collapsed = narrow;
    }
  });

  const active = $derived(tabs.find((t) => t.sessionId === activeId) ?? null);
  const connectedIds = $derived(new Set(tabs.map((t) => t.server.id)));
  // Keep <html lang> in step with the chosen language (screen readers, hyphenation).
  $effect(() => {
    document.documentElement.lang = i18n.lang;
  });

  const activeWorkspace = $derived(workspaces.find((w) => w.id === activeWsId) ?? workspaces[0] ?? null);
  const visibleServers = $derived(servers.filter((s) => workspaceIdOf(s) === activeWorkspace?.id));
  const wsCounts = $derived(
    servers.reduce<Record<string, number>>((acc, s) => {
      const id = workspaceIdOf(s);
      acc[id] = (acc[id] ?? 0) + 1;
      return acc;
    }, {}),
  );
  const groups = $derived([...new Set(visibleServers.map((s) => s.group).filter(Boolean))]);

  onMount(async () => {
    localHome = await api.localHome();
    syncBackendLanguage();
    await reloadStore();
    // Another machine changed kade.json through the sync folder.
    listen("store-changed", () => reloadStore().catch(showError));

    // Quietly look for a newer release once the window is up.
    setTimeout(async () => {
      const u = await api.updateCheck().catch(() => null);
      if (u?.newer && u.install.kind !== "other") updateAvailable = u.latest;
    }, 4000);

    // An AI assistant asked (via MCP) to open a connection.
    listen<string>("mcp-open", async ({ payload: id }) => {
      await reloadStore().catch(showError);
      const server = servers.find((s) => s.id === id);
      if (server) open(server);
    });

    // Files dropped from the OS file manager onto a server pane get uploaded.
    getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type !== "drop") return;
      const dpr = window.devicePixelRatio || 1;
      const target = targetAt(e.payload.position.x / dpr, e.payload.position.y / dpr);
      const tab = tabs.find((t) => t.sessionId === target?.sessionId);
      if (target?.side === "remote" && tab) send(tab, "upload", e.payload.paths, target.dir);
    });
  });

  // Drag between the panes of one tab: local → server uploads, server → local downloads.
  drag.onDrop = (from, to) => {
    const tab = tabs.find((t) => t.sessionId === to.sessionId);
    if (!tab || from.sessionId !== to.sessionId) return;
    send(tab, from.side === "local" ? "upload" : "download", from.paths, to.dir);
  };

  // Refresh the destination pane when a transfer ends.
  transfers.onFinished = (job) => {
    const tab = tabs.find((t) => t.sessionId === job.session_id);
    if (!tab) return;
    if (job.direction === "upload") tab.remoteRefresh++;
    else tab.localRefresh++;
    if (job.state === "failed") showError({ kind: "other", message: `${job.name}: ${job.error}` });
  };

  // After the editor saved a file, show the new size/date in the server pane.
  edits.onUploaded = (e) => {
    const tab = tabs.find((t) => t.sessionId === e.session_id);
    if (tab) tab.remoteRefresh++;
  };

  async function openFile(tab: Tab, side: "local" | "remote", entry: Entry) {
    try {
      if (side === "local") await api.openLocal(entry.path);
      else await api.editOpen(tab.sessionId, entry.path);
    } catch (e) {
      showError(e);
    }
  }

  async function send(tab: Tab, direction: Direction, paths: string[], destDir?: string) {
    const dest = destDir || (direction === "upload" ? tab.remotePath : tab.localPath);
    try {
      const names = await api.transferConflicts(tab.sessionId, direction, paths, dest);
      let policy: ConflictPolicy = "overwrite";
      if (names.length) {
        const choice = await new Promise<ConflictPolicy | null>((resolve) => (conflictAsk = { names, dest, resolve }));
        conflictAsk = null;
        if (!choice) return;
        policy = choice;
      }
      await api.transferStart(tab.sessionId, direction, paths, dest, policy);
    } catch (e) {
      showError(e);
    }
  }

  function sessionFor(serverId: string | null): string | null {
    return tabs.find((t) => t.server.id === serverId)?.sessionId ?? null;
  }

  function refreshAll() {
    for (const t of tabs) {
      t.localRefresh++;
      t.remoteRefresh++;
    }
  }

  function offerUndo(tx: Transaction, sessionId: string | null) {
    clearTimeout(undoTimer);
    undo = { tx, sessionId };
    undoTimer = setTimeout(() => (undo = null), 10_000);
  }

  async function restoreBackup(id: string) {
    try {
      const tx = (await api.backupsList()).find((t) => t.id === id);
      if (!tx) throw { kind: "other", message: t("Backup not found") };
      await api.backupRestore(id, tx.side === "remote" ? sessionFor(tx.server_id) : null);
      undo = null;
      refreshAll();
    } catch (e) {
      showError(e);
    }
  }

  function showError(e: unknown) {
    toast = errorMessage(e);
    setTimeout(() => (toast = null), 6000);
  }

  function expandHome(p: string | null): string {
    if (!p) return localHome;
    return p.startsWith("~") ? localHome + p.slice(1) : p;
  }

  async function connect(server: ServerProfile, password?: string, acceptFingerprint?: string) {
    connecting = server.id;
    try {
      const c = await api.connect(server.id, password, acceptFingerprint);
      tabs.push({
        sessionId: c.session_id,
        server,
        authLabel: c.auth_label,
        remotePath: c.home,
        localPath: expandHome(server.local_path),
        hasFiles: c.has_files,
        hasTerminal: c.has_terminal,
        view: c.has_terminal && (server.protocol === "ssh" || !c.has_files) ? "terminal" : "files",
        terminalStarted: c.has_terminal && (server.protocol === "ssh" || !c.has_files),
        localRefresh: 0,
        remoteRefresh: 0,
      });
      activeId = c.session_id;
    } catch (e) {
      const err = e as AppError;
      if (err?.kind === "host_key_unknown") {
        prompt = { kind: "hostkey", server, fingerprint: err.fingerprint, algorithm: err.algorithm, password };
      } else if (err?.kind === "password_required") {
        passwordInput = "";
        prompt = { kind: "password", server, acceptFingerprint };
      } else {
        showError(e);
      }
    } finally {
      connecting = null;
    }
  }

  function open(server: ServerProfile) {
    remember(server.id);
    const existing = tabs.find((t) => t.server.id === server.id);
    if (existing) activeId = existing.sessionId;
    else if (connecting !== server.id) connect(server);
  }

  async function closeTab(tab: Tab) {
    const i = tabs.indexOf(tab);
    tabs.splice(i, 1);
    if (activeId === tab.sessionId) activeId = tabs[Math.max(0, i - 1)]?.sessionId ?? null;
    edits.dropSession(tab.sessionId);
    await api.disconnect(tab.sessionId).catch(showError);
  }

  async function save(profile: ServerProfile, andConnect: boolean) {
    try {
      const saved = await api.saveServer(profile);
      servers = await api.listServers();
      dialog = null;
      // Keep open tabs pointing at the updated profile.
      for (const t of tabs) if (t.server.id === saved.id) t.server = saved;
      if (andConnect) open(saved);
    } catch (e) {
      showError(e);
    }
  }

  async function remove(profile: ServerProfile) {
    if (!confirm(t("Delete “{name}”?", { name: profile.name }))) return;
    try {
      for (const t of tabs.filter((t) => t.server.id === profile.id)) await closeTab(t);
      await api.deleteServer(profile.id);
      servers = await api.listServers();
      dialog = null;
    } catch (e) {
      showError(e);
    }
  }

  function trustHost() {
    if (prompt?.kind !== "hostkey") return;
    const { server, fingerprint, password } = prompt;
    prompt = null;
    connect(server, password, fingerprint);
  }

  function submitPassword() {
    if (prompt?.kind !== "password" || !passwordInput) return;
    const { server, acceptFingerprint } = prompt;
    const pw = passwordInput;
    passwordInput = "";
    prompt = null;
    connect(server, pw, acceptFingerprint);
  }

  function setView(tab: Tab, view: Tab["view"]) {
    if (view === "files" && !tab.hasFiles) return;
    if (view === "terminal" && !tab.hasTerminal) return;
    if (view === "terminal") tab.terminalStarted = true;
    tab.view = view;
  }

  function onKey(e: KeyboardEvent) {
    // Ctrl/Cmd + 1…9 switches workspace.
    if ((e.ctrlKey || e.metaKey) && /^[1-9]$/.test(e.key) && !e.shiftKey && !e.altKey) {
      const ws = workspaces[Number(e.key) - 1];
      if (ws) {
        e.preventDefault();
        switchWorkspace(ws.id);
      }
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      switcherOpen = !switcherOpen;
      return;
    }
    // Ctrl+` toggles between files and terminal, like most editors.
    if (active && e.ctrlKey && e.key === "`") {
      e.preventDefault();
      setView(active, active.view === "files" ? "terminal" : "files");
    }
  }

  function tabIcon(s: ServerProfile) {
    return s.protocol === "ssh" ? SquareTerminal : s.protocol === "sftp" ? Server : FolderUp;
  }
</script>

<svelte:window onkeydown={onKey} bind:innerWidth />

<div class="win" class:folded={collapsed}>
  <Sidebar
    {workspaces}
    {activeWorkspace}
    counts={wsCounts}
    onswitch={switchWorkspace}
    onmanage={(ws) => (wsDialog = { initial: ws, key: Date.now() })}
    bind:collapsed
    onsettings={() => (settingsOpen = true)}
    {recent}
    onsearch={() => (switcherOpen = true)}
    onbackups={() => (backupsOpen = true)}
    servers={visibleServers}
    connected={connectedIds}
    activeId={active?.server.id ?? null}
    {connecting}
    onopen={open}
    onnew={() => (dialog = { initial: null, key: Date.now() })}
    onedit={(s) => (dialog = { initial: s, key: Date.now() })}
  />

  <main class="main">
    <div class="tabs">
      {#each tabs as tab (tab.sessionId)}
        {@const TabIcon = tabIcon(tab.server)}
        <div class="tab" class:on={tab.sessionId === activeId}>
          <button class="tab-btn" onclick={() => (activeId = tab.sessionId)}>
            <span
              class="tab-ws"
              title={workspaces.find((w) => w.id === workspaceIdOf(tab.server))?.name}
              style:background={colorOf(workspaces.find((w) => w.id === workspaceIdOf(tab.server))).color}
            ></span>
            <TabIcon size={16} color={tab.sessionId === activeId ? "var(--pine)" : "var(--lichen)"} />
            {tab.server.name}
            <span class="badge">{tab.server.protocol.toUpperCase()}</span>
          </button>
          <button class="close" aria-label={t("Close")} onclick={() => closeTab(tab)}><X size={13} /></button>
        </div>
      {/each}
      <button class="add" title={t("New connection")} onclick={() => (dialog = { initial: null, key: Date.now() })}>
        <Plus size={16} color="var(--ink2)" />
      </button>
    </div>

    {#if active}
      <div class="bar">
        <div class="seg">
          <button class:on={active.view === "files"} disabled={!active.hasFiles} title={active.hasFiles ? "" : t("This server doesn't offer file transfer")} onclick={() => setView(active, "files")}>
            <Columns2 size={16} /><span class="txt">{t("Files")}</span>
          </button>
          <button
            class:on={active.view === "terminal"}
            disabled={!active.hasTerminal}
            title={active.hasTerminal ? "Terminal (Ctrl+`)" : t("FTP has no terminal; that needs SSH")}
            onclick={() => setView(active, "terminal")}
          >
            <SquareTerminal size={16} /><span class="txt">Terminal</span>
          </button>
          <span class="soon" title={t("Status — coming soon")}><Activity size={16} /><span class="txt">Status</span></span>
        </div>
        <div class="conn">
          <span class="pill" title={active.authLabel}><span class="dot"></span><span class="txt">{t("Connected")}</span></span>
          {#if active.server.protocol === "ftp"}
            <span class="pill warn" title={t("Password and files travel over the network in plain text. Use FTPS if the server supports it.")}>
              <TriangleAlert size={12} /><span class="txt">{t("Unencrypted")}</span>
            </span>
          {/if}
          {#if active.authLabel.startsWith("1Password")}<OnePasswordIcon size={14} />{/if}
          <span class="mono auth">{active.authLabel}</span>
        </div>
        <div class="right">
          <button class="btn" title={t("Disconnect")} onclick={() => closeTab(active)}>
            <Unplug size={16} color="var(--ink2)" /><span class="txt">Verbreken</span>
          </button>
        </div>
      </div>

    {/if}

    {#each tabs as tab (tab.sessionId)}
      <div class="content" class:hidden={tab.sessionId !== activeId}>
        {#if tab.hasFiles}
          <div class="panes" class:hidden={tab.view !== "files"}>
            <FilePane
              side="local"
              sessionId={tab.sessionId}
              refreshKey={tab.localRefresh}
              showHiddenDefault={settings.show_hidden}
              ops={localOps}
              label={t("Local")}
              icon={Laptop}
              bind:path={tab.localPath}
              home={localHome}
              load={api.localList}
              sendLabel={t("Upload to {name}", { name: tab.server.name })}
              onsend={(paths) => send(tab, "upload", paths)}
              ondeleted={(tx) => offerUndo(tx, null)}
              onopenfile={(entry) => openFile(tab, "local", entry)}
            />
            <FilePane
              side="remote"
              sessionId={tab.sessionId}
              refreshKey={tab.remoteRefresh}
              sendLabel={t("Download to local")}
              onsend={(paths) => send(tab, "download", paths)}
              ondeleted={(tx) => offerUndo(tx, tab.sessionId)}
              onopenfile={(entry) => openFile(tab, "remote", entry)}
              label={tab.server.name}
              showHiddenDefault={settings.show_hidden}
              icon={Server}
              accent
              bind:path={tab.remotePath}
              load={(p) => api.remoteList(tab.sessionId, p)}
              ops={remoteOps(tab.sessionId)}
              showPermissions
              footerNote={`${tab.server.user}@${tab.server.host}`}
            />
          </div>
          <div class:hidden={tab.view !== "files"}>
            <EditsBar sessionId={tab.sessionId} onerror={showError} />
            <TransferQueue sessionId={tab.sessionId} onrestore={restoreBackup} />
          </div>
        {/if}
        {#if tab.terminalStarted}
          <div class="termwrap" class:hidden={tab.view !== "terminal"}>
            <TerminalView
              sessionId={tab.sessionId}
              title={`${tab.server.user}@${tab.server.host}`}
              visible={tab.sessionId === activeId && tab.view === "terminal"}
            />
          </div>
        {/if}
      </div>
    {/each}

    {#if !active}
      <div class="empty">
        <div class="mark"><Anchor size={28} color="var(--pine)" /></div>
        <h2>{t("Pick a server to connect to")}</h2>
        <p>{t("Click a server in the sidebar, or add a new connection with 1Password or your SSH key.")}</p>
        <button class="btn pri" onclick={() => (dialog = { initial: null, key: Date.now() })}>
          <Plus size={16} />{t("New connection")}
        </button>
      </div>
    {/if}
  </main>
</div>

{#if dialog}
  {#key dialog.key}
    <ConnectDialog
      initial={dialog.initial}
      {groups}
      {workspaces}
      defaultWorkspace={activeWorkspace}
      onsave={save}
      oncancel={() => (dialog = null)}
      ondelete={remove}
    />
  {/key}
{/if}

{#if wsDialog}
  {#key wsDialog.key}
    <WorkspaceDialog
      initial={wsDialog.initial}
      {workspaces}
      counts={wsCounts}
      onsaved={async (ws) => {
        wsDialog = null;
        await reloadStore();
        switchWorkspace(ws.id);
      }}
      ondeleted={async (movedTo) => {
        wsDialog = null;
        switchWorkspace(movedTo);
        await reloadStore();
      }}
      onclose={() => (wsDialog = null)}
    />
  {/key}
{/if}

{#if switcherOpen}
  <QuickSwitcher
    {servers}
    {recent}
    {workspaces}
    activeWorkspace={activeWorkspace?.id ?? ""}
    connected={connectedIds}
    onopen={(s) => {
      // Opening something from another workspace switches to it.
      if (workspaceIdOf(s) !== activeWorkspace?.id) switchWorkspace(workspaceIdOf(s));
      open(s);
    }}
    onnew={() => (dialog = { initial: null, key: Date.now() })}
    onclose={() => (switcherOpen = false)}
  />
{/if}

{#if backupsOpen}
  <BackupsDialog
    retentionDays={settings.backup_retention_days}
    {sessionFor}
    onrestored={refreshAll}
    onclose={() => (backupsOpen = false)}
  />
{/if}

{#if conflictAsk}
  <div class="scrim" role="presentation" onclick={() => conflictAsk?.resolve(null)}></div>
  <div class="prompt" role="dialog" aria-modal="true">
    <div class="ph"><TriangleAlert size={20} color="var(--amber)" /><h3>{t("Already exists")}</h3></div>
    <p>
      {conflictAsk.names.length === 1
        ? t("{name} already exists in {dest}.", { name: conflictAsk.names[0], dest: conflictAsk.dest })
        : t("{n} items already exist in {dest}.", { n: conflictAsk.names.length, dest: conflictAsk.dest })}
      {t("Overwritten files go to Backups, so you can restore them.")}
    </p>
    <div class="pa">
      <button class="btn ghost" onclick={() => conflictAsk?.resolve(null)}>{t("Cancel")}</button>
      <button class="btn" onclick={() => conflictAsk?.resolve("skip")}>{t("Skip")}</button>
      <button class="btn" onclick={() => conflictAsk?.resolve("newer")}>{t("Only newer")}</button>
      <button class="btn pri" onclick={() => conflictAsk?.resolve("overwrite")}>{t("Overwrite")}</button>
    </div>
  </div>
{/if}

{#if settingsOpen}
  <SettingsDialog {settings} onchange={() => reloadStore().catch(showError)} onclose={() => (settingsOpen = false)} />
{/if}

{#if prompt}
  <div class="scrim" role="presentation" onclick={() => (prompt = null)}></div>
  <div class="prompt" role="dialog" aria-modal="true">
    {#if prompt.kind === "hostkey"}
      <div class="ph"><Fingerprint size={20} color="var(--amber)" /><h3>{t("Unknown server")}</h3></div>
      <p>
        {t("Kade hasn't seen {host} before. Check the fingerprint before you continue — on the server, run:", { host: prompt.server.host })}
        <span class="mono">ssh-keygen -lf /etc/ssh/ssh_host_*_key.pub</span>
      </p>
      <div class="fp mono"><small>{prompt.algorithm}</small>{prompt.fingerprint}</div>
      <div class="pa">
        <button class="btn ghost" onclick={() => (prompt = null)}>{t("Cancel")}</button>
        <button class="btn pri" onclick={trustHost}>{t("Trust & connect")}</button>
      </div>
    {:else}
      <div class="ph"><Fingerprint size={20} color="var(--pine)" /><h3>{prompt.server.auth.method === "key_file" ? t("Key passphrase") : t("Password")}</h3></div>
      <p>{t("For {who}. Not stored.", { who: `${prompt.server.user}@${prompt.server.host}` })}</p>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="pw"
        type="password"
        bind:value={passwordInput}
        autofocus
        onkeydown={(e) => e.key === "Enter" && submitPassword()}
      />
      <div class="pa">
        <button class="btn ghost" onclick={() => (prompt = null)}>{t("Cancel")}</button>
        <button class="btn pri" disabled={!passwordInput} onclick={submitPassword}>{t("Connect")}</button>
      </div>
    {/if}
  </div>
{/if}

{#if connecting}
  <div class="busy"><LoaderCircle size={14} class="spin" />{t("Connecting to {name}…", { name: servers.find((s) => s.id === connecting)?.name ?? "" })}</div>
{/if}

{#if drag.source}
  <div class="drag-ghost" style:left="{drag.x + 14}px" style:top="{drag.y + 10}px">
    {drag.source.paths.length === 1 ? drag.source.paths[0].split("/").pop() : `${drag.source.paths.length} items`}
    {#if drag.over && drag.over.side !== drag.source.side}
      <span>→ {drag.over.side === "remote" ? t("upload") : t("download")}</span>
    {/if}
  </div>
{/if}

{#if updateAvailable && !undo && !toast}
  <div class="toast undo" role="status">
    <span>{t("Kade {v} is available", { v: updateAvailable })}</span>
    <button onclick={() => ((settingsOpen = true), (updateAvailable = null))}>{t("Update")}</button>
    <button aria-label={t("Later")} onclick={() => (updateAvailable = null)}><X size={14} /></button>
  </div>
{/if}

{#if undo && !toast}
  <div class="toast undo" role="status">
    <span>{undo.tx.summary}</span>
    <button onclick={() => undo && restoreBackup(undo.tx.id)}><Undo2 size={14} />{t("Undo")}</button>
  </div>
{/if}

{#if toast}
  <div class="toast" role="alert"><TriangleAlert size={16} color="#fff" />{toast}</div>
{/if}

<style>
  .win {
    height: 100vh;
    display: grid;
    grid-template-columns: 248px minmax(0, 1fr);
  }
  .win.folded {
    grid-template-columns: 64px minmax(0, 1fr);
  }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    container: main / inline-size;
  }

  .tabs {
    height: 44px;
    flex: none;
    display: flex;
    align-items: flex-end;
    gap: 2px;
    padding: 0 14px;
    border-bottom: 1px solid var(--mist);
    background: var(--rail);
    overflow-x: auto;
  }
  .tab {
    display: flex;
    align-items: center;
    height: 34px;
    padding: 0 6px 0 14px;
    border-radius: 8px 8px 0 0;
    color: var(--ink2);
    font-weight: 500;
    flex: none;
  }
  .tab.on {
    background: var(--snow);
    color: var(--granite);
    box-shadow: 0 0 0 1px var(--mist);
    margin-bottom: -1px;
    height: 35px;
  }
  .tab-btn {
    max-width: 220px;
    overflow: hidden;
    white-space: nowrap;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 100%;
  }
  .tab-ws {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex: none;
  }
  .badge {
    font: 500 10px var(--mono);
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--mist2);
    color: var(--ink2);
  }
  .tab.on .badge {
    background: var(--pine-t);
    color: var(--pine);
  }
  .close {
    margin-left: 6px;
    padding: 3px;
    border-radius: 4px;
    opacity: 0.45;
    display: grid;
  }
  .close:hover {
    opacity: 1;
    background: var(--mist2);
  }
  .add {
    margin: 0 0 8px 6px;
    padding: 3px;
    border-radius: 6px;
    display: grid;
  }
  .add:hover {
    background: var(--mist2);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 18px;
    flex: none;
  }
  .seg {
    display: flex;
    background: var(--mist2);
    border-radius: 8px;
    padding: 2px;
  }
  .seg > span,
  .seg > button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 11px;
    border-radius: 6px;
    font-weight: 500;
    color: var(--ink2);
  }
  .seg button.on {
    background: var(--paper);
    color: var(--granite);
    box-shadow: 0 1px 2px rgba(30, 35, 33, 0.08);
  }
  .seg > span.soon,
  .seg button:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .content {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .termwrap {
    flex: 1;
    display: flex;
    padding: 0 18px 16px;
    min-height: 0;
  }
  .hidden {
    display: none !important;
  }
  .conn {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--ink2);
    font-size: 12px;
    min-width: 0;
  }
  .conn .mono {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 9px;
    border-radius: 99px;
    background: var(--pine-t);
    color: var(--pine);
    font-weight: 600;
    font-size: 11.5px;
    flex: none;
  }
  .pill.warn {
    background: var(--amber-t);
    color: var(--amber);
  }
  .pill .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--pine);
  }
  .right {
    margin-left: auto;
    display: flex;
    gap: 8px;
  }
  .panes {
    flex: 1;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    padding: 0 18px 16px;
    min-height: 0;
  }
  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    text-align: center;
    padding: 40px;
  }
  .empty .mark {
    width: 56px;
    height: 56px;
    border-radius: 14px;
    background: var(--pine-t);
    display: grid;
    place-items: center;
    margin-bottom: 6px;
  }
  .empty h2 {
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.015em;
  }
  .empty p {
    color: var(--lichen);
    max-width: 380px;
    margin-bottom: 8px;
  }
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(30, 35, 33, 0.32);
    backdrop-filter: blur(3px);
    z-index: 20;
  }
  .prompt {
    position: fixed;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    width: min(480px, calc(100vw - 40px));
    background: var(--paper);
    border-radius: 14px;
    padding: 22px 24px 18px;
    box-shadow: 0 30px 80px rgba(15, 21, 19, 0.28);
    z-index: 21;
    display: flex;
    flex-direction: column;
    gap: 12px;
    user-select: text;
  }
  .prompt .ph {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .prompt h3 {
    font-size: 16px;
    font-weight: 600;
  }
  .prompt p {
    color: var(--ink2);
    line-height: 1.5;
  }
  .prompt p .mono {
    font-size: 11.5px;
    background: var(--mist2);
    padding: 1px 4px;
    border-radius: 4px;
  }
  .fp {
    background: var(--snow);
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 10px 12px;
    font-size: 12px;
    word-break: break-all;
  }
  .fp small {
    display: block;
    color: var(--lichen);
    font-size: 11px;
    margin-bottom: 2px;
  }
  .pw {
    height: 36px;
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 0 11px;
    outline: 0;
  }
  .pw:focus {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
  }
  .pa {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
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
    box-shadow: 0 10px 30px rgba(15, 21, 19, 0.12);
  }
  .drag-ghost {
    position: fixed;
    z-index: 50;
    pointer-events: none;
    background: var(--granite);
    color: #fff;
    padding: 6px 10px;
    border-radius: 8px;
    font-weight: 500;
    font-size: 12.5px;
    box-shadow: 0 10px 30px rgba(15, 21, 19, 0.25);
    white-space: nowrap;
  }
  .drag-ghost span {
    opacity: 0.7;
    margin-left: 6px;
    font-family: var(--mono);
    font-size: 11px;
  }
  .toast.undo {
    background: var(--granite);
  }
  .toast.undo button {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 6px;
    padding: 4px 9px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.14);
    color: #fff;
    font-weight: 600;
  }
  .toast.undo button:hover {
    background: rgba(255, 255, 255, 0.24);
  }
  .toast {
    background: var(--danger);
    color: #fff;
    box-shadow: 0 10px 30px rgba(15, 21, 19, 0.25);
  }
  /* Narrow main area (half-screen): icon-only toolbar, stacked panes. */
  @container main (max-width: 900px) {
    .bar .auth {
      display: none;
    }
  }
  @container main (max-width: 760px) {
    .bar {
      padding: 10px 12px;
      gap: 8px;
    }
    .bar .txt {
      display: none;
    }
    .seg > button,
    .seg > span {
      padding: 5px 8px;
    }
    .pill {
      padding: 5px;
    }
    .panes {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: minmax(0, 1fr) minmax(0, 1fr);
      padding: 0 12px 12px;
      gap: 10px;
    }
    .termwrap {
      padding: 0 12px 12px;
    }
    .tabs {
      padding: 0 8px;
    }
  }
</style>
