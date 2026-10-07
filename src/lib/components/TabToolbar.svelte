<script lang="ts">
  import { Activity, Cable, Columns2, SquareTerminal, TriangleAlert, Unplug } from "@lucide/svelte";
  import { tabs, type Tab } from "$lib/tabs.svelte";
  import { t } from "$lib/i18n.svelte";
  import OnePasswordIcon from "./OnePasswordIcon.svelte";

  let { tab }: { tab: Tab } = $props();
</script>

<div class="bar">
  <div class="seg">
    <button class:on={tab.view === "files"} disabled={!tab.hasFiles} title={tab.hasFiles ? "" : t("This server doesn't offer file transfer")} onclick={() => tabs.setView(tab, "files")}>
      <Columns2 size={16} /><span class="txt">{t("Files")}</span>
    </button>
    <button
      class:on={tab.view === "terminal"}
      disabled={!tab.hasTerminal}
      title={tab.hasTerminal ? "Terminal (Ctrl+`)" : t("FTP has no terminal; that needs SSH")}
      onclick={() => tabs.setView(tab, "terminal")}
    >
      <SquareTerminal size={16} /><span class="txt">Terminal</span>
    </button>
    <button
      class:on={tab.view === "tunnels"}
      disabled={!tab.hasTerminal}
      title={tab.hasTerminal ? t("Tunnels") : t("Tunnels need SSH")}
      onclick={() => tabs.setView(tab, "tunnels")}
    >
      <Cable size={16} /><span class="txt">{t("Tunnels")}</span>
      {#if tab.server.tunnels?.length}<span class="count">{tab.server.tunnels.length}</span>{/if}
    </button>
    <button
      class:on={tab.view === "status"}
      disabled={!tab.hasTerminal}
      title={tab.hasTerminal ? t("CPU, memory and disks of the server") : t("Status needs SSH")}
      onclick={() => tabs.setView(tab, "status")}
    >
      <Activity size={16} /><span class="txt">{t("Status")}</span>
    </button>
  </div>
  <div class="conn">
    <span class="pill" title={tab.authLabel}><span class="dot"></span><span class="txt">{t("Connected")}</span></span>
    {#if tab.server.protocol === "ftp"}
      <span class="pill warn" title={t("Password and files travel over the network in plain text. Use FTPS if the server supports it.")}>
        <TriangleAlert size={12} /><span class="txt">{t("Unencrypted")}</span>
      </span>
    {/if}
    {#if tab.authLabel.startsWith("1Password")}<OnePasswordIcon size={14} />{/if}
    <span class="mono auth">{tab.authLabel}</span>
  </div>
  <div class="right">
    <button class="btn" title={t("Disconnect")} onclick={() => tabs.close(tab)}>
      <Unplug size={16} color="var(--ink2)" /><span class="txt">{t("Disconnect")}</span>
    </button>
  </div>
</div>

<style>
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
    box-shadow: var(--shadow-sm);
  }
  .seg .count {
    font: 600 10px var(--mono);
    padding: 0 5px;
    border-radius: 99px;
    background: var(--mist);
    color: var(--ink2);
  }
  .seg button.on .count {
    background: var(--pine-t);
    color: var(--pine);
  }
  .seg button:disabled {
    opacity: 0.45;
    cursor: not-allowed;
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
  /* Narrow main area (half-screen): icon-only toolbar. */
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
    .seg > button {
      padding: 5px 8px;
    }
    .pill {
      padding: 5px;
    }
  }
</style>
