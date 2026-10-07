<script lang="ts">
  import { onMount } from "svelte";
  import { Anchor, Plus } from "@lucide/svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { listen } from "@tauri-apps/api/event";
  import { api, type ServerProfile, type Workspace } from "$lib/api";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import TabBar from "$lib/components/TabBar.svelte";
  import TabToolbar from "$lib/components/TabToolbar.svelte";
  import TabView from "$lib/components/TabView.svelte";
  import ConnectDialog from "$lib/components/ConnectDialog.svelte";
  import SettingsPage from "$lib/components/SettingsPage.svelte";
  import BackupsDialog from "$lib/components/BackupsDialog.svelte";
  import ImportDialog from "$lib/components/ImportDialog.svelte";
  import QuickSwitcher from "$lib/components/QuickSwitcher.svelte";
  import WorkspaceDialog from "$lib/components/WorkspaceDialog.svelte";
  import ConflictPrompt from "$lib/components/ConflictPrompt.svelte";
  import ConnectPrompt from "$lib/components/ConnectPrompt.svelte";
  import Toasts from "$lib/components/Toasts.svelte";
  import DragGhost from "$lib/components/DragGhost.svelte";
  import { workspaceIdOf } from "$lib/workspaces";
  import { i18n, syncBackendLanguage, t, tn } from "$lib/i18n.svelte";
  import { edits } from "$lib/edits.svelte";
  import { transfers } from "$lib/transfers.svelte";
  import { drag, targetAt } from "$lib/drag.svelte";
  import { store } from "$lib/store.svelte";
  import { tabs } from "$lib/tabs.svelte";
  import { showError, toasts } from "$lib/toasts.svelte";

  let dialog = $state<{ initial: ServerProfile | null; key: number } | null>(null);
  let wsDialog = $state<{ initial: Workspace | null; key: number } | null>(null);
  /** The settings page, open on a section (null: the top). */
  let settings = $state<{ section: string | null } | null>(null);
  let importOpen = $state(false);
  let backupsOpen = $state(false);
  let switcherOpen = $state(false);

  const newConnection = () => (dialog = { initial: null, key: Date.now() });

  // Opening or switching to a connection brings its tab back into view.
  $effect(() => {
    if (tabs.activeId) settings = null;
  });

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

  // Keep <html lang> in step with the chosen language (screen readers, hyphenation).
  $effect(() => {
    document.documentElement.lang = i18n.lang;
  });

  onMount(async () => {
    tabs.localHome = await api.localHome().catch((e) => (showError(e), tabs.localHome));
    syncBackendLanguage();
    await store.reload().catch(showError);
    // Another machine changed kade.json through the sync folder.
    listen("store-changed", () => store.reload().catch(showError));

    // Quietly look for a newer release once the window is up.
    setTimeout(async () => {
      const u = await api.updateCheck().catch(() => null);
      if (u?.newer && u.install.kind !== "other") toasts.update = u.latest;
    }, 4000);

    // An AI assistant asked (via MCP) to open a connection.
    listen<string>("mcp-open", async ({ payload: id }) => {
      await store.reload().catch(showError);
      const server = store.servers.find((s) => s.id === id);
      if (server) tabs.open(server);
    });

    // Files dropped from the OS file manager onto a server pane get uploaded.
    getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type !== "drop") return;
      const dpr = window.devicePixelRatio || 1;
      const target = targetAt(e.payload.position.x / dpr, e.payload.position.y / dpr);
      const tab = tabs.find(target?.sessionId);
      if (target?.side === "remote" && tab) tabs.send(tab, "upload", e.payload.paths, target.dir);
    });
  });

  // Drag between the panes of one tab: local → server uploads, server → local downloads.
  drag.onDrop = (from, to) => {
    const tab = tabs.find(to.sessionId);
    if (!tab || from.sessionId !== to.sessionId) return;
    tabs.send(tab, from.side === "local" ? "upload" : "download", from.paths, to.dir);
  };

  // Refresh the destination pane when a transfer ends.
  transfers.onFinished = (job) => {
    const tab = tabs.find(job.session_id);
    if (!tab) return;
    if (job.direction === "upload") tab.remoteRefresh++;
    else tab.localRefresh++;
    if (job.state === "failed") showError({ kind: "other", message: `${job.name}: ${job.error}` });
  };

  // After the editor saved a file, show the new size/date in the server pane.
  edits.onUploaded = (e) => {
    const tab = tabs.find(e.session_id);
    if (tab) tab.remoteRefresh++;
  };

  async function restoreBackup(id: string) {
    try {
      const tx = (await api.backupsList()).find((t) => t.id === id);
      if (!tx) throw { kind: "other", message: t("Backup not found") };
      await api.backupRestore(id, tx.side === "remote" ? tabs.sessionFor(tx.server_id) : null);
      toasts.undo = null;
      tabs.refreshAll();
    } catch (e) {
      showError(e);
    }
  }

  async function save(profile: ServerProfile, andConnect: boolean) {
    try {
      const saved = await tabs.saveServer(profile);
      dialog = null;
      if (andConnect) tabs.open(saved);
    } catch (e) {
      showError(e);
    }
  }

  async function remove(profile: ServerProfile) {
    if (!confirm(t("Delete “{name}”?", { name: profile.name }))) return;
    try {
      for (const t of tabs.list.filter((t) => t.server.id === profile.id)) await tabs.close(t);
      await api.deleteServer(profile.id);
      store.servers = await api.listServers();
      dialog = null;
    } catch (e) {
      showError(e);
    }
  }

  function onKey(e: KeyboardEvent) {
    // Ctrl/Cmd + 1…9 switches workspace.
    if ((e.ctrlKey || e.metaKey) && /^[1-9]$/.test(e.key) && !e.shiftKey && !e.altKey) {
      const ws = store.workspaces[Number(e.key) - 1];
      if (ws) {
        e.preventDefault();
        store.switchWorkspace(ws.id);
      }
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      switcherOpen = !switcherOpen;
      return;
    }
    // Ctrl+` toggles between files and terminal, like most editors.
    const active = tabs.active;
    if (active && e.ctrlKey && e.key === "`") {
      e.preventDefault();
      tabs.setView(active, active.view === "files" ? "terminal" : "files");
    }
  }
</script>

<svelte:window onkeydown={onKey} bind:innerWidth />

<div class="win" class:folded={collapsed}>
  <Sidebar
    workspaces={store.workspaces}
    activeWorkspace={store.activeWorkspace}
    counts={store.counts}
    onswitch={(id) => store.switchWorkspace(id)}
    onmanage={(ws) => (wsDialog = { initial: ws, key: Date.now() })}
    bind:collapsed
    onsettings={() => (settings = settings ? null : { section: null })}
    onimport={() => (importOpen = true)}
    recent={store.recent}
    onsearch={() => (switcherOpen = true)}
    onbackups={() => (backupsOpen = true)}
    servers={store.visibleServers}
    connected={tabs.connectedIds}
    activeId={tabs.active?.server.id ?? null}
    connecting={tabs.connecting}
    onopen={(s) => ((settings = null), tabs.open(s))}
    onnew={newConnection}
    onedit={(s) => (dialog = { initial: s, key: Date.now() })}
  />

  <main class="main">
    <TabBar onnew={newConnection} />

    {#if tabs.active}
      <TabToolbar tab={tabs.active} />
    {/if}

    {#each tabs.list as tab (tab.sessionId)}
      <TabView {tab} onrestore={restoreBackup} />
    {/each}

    {#if !tabs.active}
      <div class="empty">
        <div class="mark"><Anchor size={28} color="var(--pine)" /></div>
        <h2>{t("Pick a server to connect to")}</h2>
        <p>{t("Click a server in the sidebar, or add a new connection with 1Password or your SSH key.")}</p>
        <button class="btn pri" onclick={newConnection}>
          <Plus size={16} />{t("New connection")}
        </button>
      </div>
    {/if}
  </main>

  {#if settings}
    <!-- Over the main area, so the tabs (and their terminals) stay as they are. -->
    <SettingsPage
      settings={store.settings}
      section={settings.section}
      onchange={() => store.reload().catch(showError)}
      onimport={() => ((settings = null), (importOpen = true))}
      onclose={() => (settings = null)}
    />
  {/if}
</div>

{#if dialog}
  {#key dialog.key}
    <ConnectDialog
      initial={dialog.initial}
      groups={store.groups}
      workspaces={store.workspaces}
      defaultWorkspace={store.activeWorkspace}
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
      workspaces={store.workspaces}
      counts={store.counts}
      onsaved={async (ws) => {
        wsDialog = null;
        await store.reload();
        store.switchWorkspace(ws.id);
      }}
      ondeleted={async (movedTo) => {
        wsDialog = null;
        store.switchWorkspace(movedTo);
        await store.reload();
      }}
      onclose={() => (wsDialog = null)}
    />
  {/key}
{/if}

{#if switcherOpen}
  <QuickSwitcher
    servers={store.servers}
    recent={store.recent}
    workspaces={store.workspaces}
    activeWorkspace={store.activeWorkspace?.id ?? ""}
    connected={tabs.connectedIds}
    onopen={(s) => {
      // Opening something from another workspace switches to it.
      if (workspaceIdOf(s) !== store.activeWorkspace?.id) store.switchWorkspace(workspaceIdOf(s));
      tabs.open(s);
    }}
    onnew={newConnection}
    onclose={() => (switcherOpen = false)}
  />
{/if}

{#if backupsOpen}
  <BackupsDialog
    retentionDays={store.settings.backup_retention_days}
    sessionFor={tabs.sessionFor}
    onrestored={() => tabs.refreshAll()}
    onclose={() => (backupsOpen = false)}
  />
{/if}

{#if tabs.conflict}
  <ConflictPrompt ask={tabs.conflict} />
{/if}

{#if importOpen}
  <ImportDialog
    workspaces={store.workspaces}
    defaultWorkspace={store.activeWorkspace}
    onimported={async (ws, n) => {
      importOpen = false;
      await store.reload().catch(showError);
      store.switchWorkspace(ws);
      toasts.showNotice(tn(n, "Imported {n} connection", "Imported {n} connections"));
    }}
    onclose={() => (importOpen = false)}
  />
{/if}

{#if tabs.prompt}
  {#key tabs.prompt}
    <ConnectPrompt
      prompt={tabs.prompt}
      onclose={() => (tabs.prompt = null)}
      onconnect={(server, password, fingerprint) => tabs.connect(server, password, fingerprint)}
    />
  {/key}
{/if}

<Toasts onundo={restoreBackup} onupdate={() => (settings = { section: "about" })} />
<DragGhost />

<style>
  .win {
    height: 100vh;
    display: grid;
    grid-template-columns: 248px minmax(0, 1fr);
  }
  .win.folded {
    grid-template-columns: 64px minmax(0, 1fr);
  }
  .main,
  .win > :global(.page) {
    grid-area: 1 / 2;
  }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    container: main / inline-size;
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
</style>
