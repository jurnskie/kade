<script lang="ts">
  import { Laptop, Server } from "@lucide/svelte";
  import { api, errorMessage, joinPath, localOps, remoteOps, type Entry, type RsyncSupport, type SyncDirection } from "$lib/api";
  import { tabs, type Tab } from "$lib/tabs.svelte";
  import { store } from "$lib/store.svelte";
  import { showError, toasts } from "$lib/toasts.svelte";
  import { t } from "$lib/i18n.svelte";
  import FilePane from "./FilePane.svelte";
  import EditsBar from "./EditsBar.svelte";
  import TransferQueue from "./TransferQueue.svelte";
  import TunnelsView from "./TunnelsView.svelte";
  import StatusView from "./StatusView.svelte";
  import TerminalView from "./TerminalView.svelte";
  import SyncDialog from "./SyncDialog.svelte";

  let { tab = $bindable(), onrestore }: { tab: Tab; onrestore: (backupId: string) => void } = $props();

  const visible = $derived(tab.sessionId === tabs.activeId);

  // Whether rsync works for this connection; asked once, the first time a context menu opens.
  let support = $state<RsyncSupport | null>(null);
  let supportAsked = false;
  let supportError = $state<string | null>(null);
  let syncing = $state<{ direction: SyncDirection; local: string; remote: string } | null>(null);

  function checkSupport() {
    if (supportAsked) return;
    supportAsked = true;
    api
      .rsyncSupport(tab.sessionId)
      .then((s) => (support = s))
      .catch((e) => (supportError = errorMessage(e)));
  }

  /** null when sync can be used; otherwise the reason, for the disabled menu item's tooltip. */
  const syncReason = $derived(supportError ?? (support ? (support.available ? null : (support.reason ?? t("Sync isn't available here"))) : t("Checking for rsync…")));

  /** The other pane's current folder plus this folder's name, like a drop. */
  function sync(direction: SyncDirection, folder: string) {
    const name = folder.replace(/\/+$/, "").split("/").pop() ?? "";
    syncing =
      direction === "to_server"
        ? { direction, local: folder, remote: name ? joinPath(tab.remotePath, name) : tab.remotePath }
        : { direction, local: name ? joinPath(tab.localPath, name) : tab.localPath, remote: folder };
  }

  async function openFile(side: "local" | "remote", entry: Entry) {
    try {
      if (side === "local") await api.openLocal(entry.path);
      else await api.editOpen(tab.sessionId, entry.path);
    } catch (e) {
      showError(e);
    }
  }
</script>

<div class="content" class:hidden={!visible}>
  {#if tab.hasFiles}
    <div class="panes" class:hidden={tab.view !== "files"}>
      <FilePane
        side="local"
        sessionId={tab.sessionId}
        refreshKey={tab.localRefresh}
        showHiddenDefault={store.settings.show_hidden}
        ops={localOps}
        label={t("Local")}
        icon={Laptop}
        bind:path={tab.localPath}
        home={tabs.localHome}
        load={api.localList}
        sendLabel={t("Upload to {name}", { name: tab.server.name })}
        onsend={(paths) => tabs.send(tab, "upload", paths)}
        syncLabel={t("Sync this folder to server…")}
        {syncReason}
        onsync={(folder) => sync("to_server", folder)}
        onmenuopen={checkSupport}
        ondeleted={(tx) => toasts.offerUndo(tx, null)}
        onopenfile={(entry) => openFile("local", entry)}
      />
      <FilePane
        side="remote"
        sessionId={tab.sessionId}
        refreshKey={tab.remoteRefresh}
        sendLabel={t("Download to local")}
        onsend={(paths) => tabs.send(tab, "download", paths)}
        syncLabel={t("Sync this folder from server…")}
        {syncReason}
        onsync={(folder) => sync("from_server", folder)}
        onmenuopen={checkSupport}
        ondeleted={(tx) => toasts.offerUndo(tx, tab.sessionId)}
        onopenfile={(entry) => openFile("remote", entry)}
        label={tab.server.name}
        showHiddenDefault={store.settings.show_hidden}
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
      <TransferQueue sessionId={tab.sessionId} {onrestore} onerror={showError} />
    </div>
  {/if}
  {#if syncing}
    <SyncDialog
      sessionId={tab.sessionId}
      serverName={tab.server.name}
      {...syncing}
      pullWarning={support?.pull_warning}
      onclose={() => (syncing = null)}
    />
  {/if}
  {#if tab.hasTerminal}
    <div class="pagewrap" class:hidden={tab.view !== "tunnels"}>
      <TunnelsView sessionId={tab.sessionId} server={tab.server} onsave={tabs.saveServer} onerror={showError} />
    </div>
    {#if tab.view === "status"}
      <div class="pagewrap">
        <StatusView sessionId={tab.sessionId} {visible} />
      </div>
    {/if}
  {/if}
  {#if tab.terminalStarted}
    <div class="termwrap" class:hidden={tab.view !== "terminal"}>
      <TerminalView
        sessionId={tab.sessionId}
        title={`${tab.server.user}@${tab.server.host}`}
        visible={visible && tab.view === "terminal"}
      />
    </div>
  {/if}
</div>

<style>
  .content {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .panes {
    flex: 1;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    padding: 0 18px 16px;
    min-height: 0;
  }
  .pagewrap {
    flex: 1;
    display: flex;
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
  /* Narrow main area (half-screen): stacked panes. */
  @container main (max-width: 760px) {
    .panes {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: minmax(0, 1fr) minmax(0, 1fr);
      padding: 0 12px 12px;
      gap: 10px;
    }
    .termwrap {
      padding: 0 12px 12px;
    }
  }
</style>
