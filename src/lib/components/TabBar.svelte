<script lang="ts">
  import { FolderUp, Plus, Server, SquareTerminal, X } from "@lucide/svelte";
  import type { ServerProfile } from "$lib/api";
  import { tabs } from "$lib/tabs.svelte";
  import { store } from "$lib/store.svelte";
  import { colorOf, workspaceIdOf } from "$lib/workspaces";
  import { t } from "$lib/i18n.svelte";

  let { onnew }: { onnew: () => void } = $props();

  function tabIcon(s: ServerProfile) {
    return s.protocol === "ssh" ? SquareTerminal : s.protocol === "sftp" ? Server : FolderUp;
  }
</script>

<div class="tabs">
  {#each tabs.list as tab (tab.sessionId)}
    {@const TabIcon = tabIcon(tab.server)}
    {@const ws = store.workspaces.find((w) => w.id === workspaceIdOf(tab.server))}
    <div class="tab" class:on={tab.sessionId === tabs.activeId}>
      <button class="tab-btn" onclick={() => (tabs.activeId = tab.sessionId)}>
        <span class="tab-ws" title={ws?.name} style:background={colorOf(ws).color}></span>
        <TabIcon size={16} color={tab.sessionId === tabs.activeId ? "var(--pine)" : "var(--lichen)"} />
        {tab.server.name}
        <span class="badge">{tab.server.protocol.toUpperCase()}</span>
      </button>
      <button class="close" aria-label={t("Close")} onclick={() => tabs.close(tab)}><X size={13} /></button>
    </div>
  {/each}
  <button class="add" title={t("New connection")} onclick={onnew}>
    <Plus size={16} color="var(--ink2)" />
  </button>
</div>

<style>
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
    /* The active tab overlaps the border by 1px, which would otherwise add a vertical scrollbar. */
    overflow-y: hidden;
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
  @container main (max-width: 760px) {
    .tabs {
      padding: 0 8px;
    }
  }
</style>
