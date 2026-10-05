<script lang="ts">
  import {
    Anchor,
    Search,
    Server,
    SquareTerminal,
    FolderUp,
    Plus,
    Pencil,
    PanelLeftClose,
    Settings,
    ArchiveRestore,
    ChevronRight,
    ChevronsUpDown,
    Clock,
    Check,
  } from "@lucide/svelte";
  import type { ServerProfile, Workspace } from "$lib/api";
  import { colorOf } from "$lib/workspaces";
  import { locale, t } from "$lib/i18n.svelte";

  let {
    servers,
    connected,
    activeId,
    connecting,
    recent,
    onopen,
    onnew,
    onedit,
    onsearch,
    collapsed = $bindable(false),
    onsettings,
    onimport,
    onbackups,
    workspaces,
    activeWorkspace,
    counts,
    onswitch,
    onmanage,
  }: {
    servers: ServerProfile[];
    connected: Set<string>;
    activeId: string | null;
    connecting: string | null;
    /** Ids of recently opened connections on this machine, newest first. */
    recent: string[];
    onopen: (s: ServerProfile) => void;
    onnew: () => void;
    onedit: (s: ServerProfile) => void;
    onsearch: () => void;
    collapsed?: boolean;
    onsettings: () => void;
    /** Open the import dialog (Cyberduck, FileZilla, Transmit). */
    onimport: () => void;
    onbackups: () => void;
    workspaces: Workspace[];
    activeWorkspace: Workspace | null;
    /** Connections per workspace id. */
    counts: Record<string, number>;
    onswitch: (id: string) => void;
    /** Edit a workspace, or create one with null. */
    onmanage: (ws: Workspace | null) => void;
  } = $props();

  let wsMenu = $state(false);
  const wsColor = $derived(colorOf(activeWorkspace));

  // Folded groups are remembered per machine (a convenience, so storage may fail).
  const FOLD_KEY = "kade.foldedGroups";
  let folded = $state<Set<string>>(
    (() => {
      try {
        return new Set<string>(JSON.parse(localStorage.getItem(FOLD_KEY) ?? "[]"));
      } catch {
        return new Set<string>();
      }
    })(),
  );

  function toggle(group: string) {
    const next = new Set(folded);
    next.has(group) ? next.delete(group) : next.add(group);
    folded = next;
    try {
      localStorage.setItem(FOLD_KEY, JSON.stringify([...next]));
    } catch {
      /* not persisted; fine */
    }
  }

  const groups = $derived.by(() => {
    const map = new Map<string, ServerProfile[]>();
    for (const s of servers) {
      const g = s.group.trim() || t("Other");
      map.set(g, [...(map.get(g) ?? []), s]);
    }
    for (const list of map.values()) list.sort((a, b) => a.name.localeCompare(b.name, locale()));
    return [...map.entries()].sort(([a], [b]) => a.localeCompare(b, locale()));
  });

  // Recent only earns its space once the list is long enough to scroll.
  const recentServers = $derived(
    servers.length > 6
      ? recent.map((id) => servers.find((s) => s.id === id)).filter((s): s is ServerProfile => !!s).slice(0, 5)
      : [],
  );

  const groupHasActivity = (items: ServerProfile[]) => items.some((s) => connected.has(s.id) || s.id === activeId);
</script>

{#snippet row(s: ServerProfile)}
  <div
    class="srv"
    class:on={s.id === activeId}
    title={collapsed ? `${s.name} — ${s.user}@${s.host}` : `${s.protocol} · ${s.user}@${s.host}${s.remote_path ? ` · ${s.remote_path}` : ""}`}
    role="button"
    tabindex="0"
    onclick={() => onopen(s)}
    onkeydown={(e) => e.key === "Enter" && onopen(s)}
  >
    <span class="ic">
      {#if s.protocol === "ssh"}
        <SquareTerminal size={15} color={s.id === activeId ? "var(--pine)" : "var(--ink2)"} />
      {:else if s.protocol === "sftp"}
        <Server size={15} color={s.id === activeId ? "var(--pine)" : "var(--ink2)"} />
      {:else}
        <FolderUp size={15} color={s.id === activeId ? "var(--pine)" : "var(--ink2)"} />
      {/if}
    </span>
    <span class="name">{s.name}</span>
    <span class="host mono">{s.host}</span>
    <button class="edit" title={t("Edit")} onclick={(e) => (e.stopPropagation(), onedit(s))}>
      <Pencil size={12} color="var(--lichen)" />
    </button>
    <span class="dot" class:off={!connected.has(s.id)} class:busy={connecting === s.id}></span>
  </div>
{/snippet}

<svelte:window onclick={() => (wsMenu = false)} onkeydown={(e) => e.key === "Escape" && (wsMenu = false)} />

<aside class="side" class:rail={collapsed}>
  <div class="brand">
    <button class="mark" title={collapsed ? t("Expand sidebar") : "Kade"} onclick={() => (collapsed = false)}>
      <Anchor size={16} color="var(--on-pine)" />
    </button>
    {#if !collapsed}
      <b>Kade</b>
      <button class="fold" title={t("Collapse sidebar")} onclick={() => (collapsed = true)}>
        <PanelLeftClose size={16} color="var(--lichen)" />
      </button>
    {/if}
  </div>

  <div class="ws-wrap">
    <button
      class="ws"
      style:--ws={wsColor.color}
      style:--ws-tint={wsColor.tint}
      title={collapsed ? t("Workspace: {name}", { name: activeWorkspace?.name ?? "" }) : t("Switch workspace")}
      onclick={(e) => (e.stopPropagation(), (wsMenu = !wsMenu))}
    >
      <span class="ws-dot">{(activeWorkspace?.name ?? "?").slice(0, 1).toUpperCase()}</span>
      <span class="ws-name">{activeWorkspace?.name ?? ""}</span>
      <ChevronsUpDown size={14} color="var(--lichen)" />
    </button>
    {#if wsMenu}
      <div class="ws-menu" role="menu">
        {#each workspaces as w, i (w.id)}
          {@const c = colorOf(w)}
          <div class="ws-item" class:on={w.id === activeWorkspace?.id}>
            <button class="ws-pick" role="menuitem" onclick={() => ((wsMenu = false), onswitch(w.id))}>
              <span class="ws-dot small" style:--ws={c.color}>{w.name.slice(0, 1).toUpperCase()}</span>
              <span class="ws-label">{w.name}</span>
              <span class="ws-count">{counts[w.id] ?? 0}</span>
              {#if i < 9}<kbd>Ctrl {i + 1}</kbd>{/if}
              {#if w.id === activeWorkspace?.id}<Check size={14} color="var(--pine)" />{/if}
            </button>
            <button class="ws-edit" title={t("Edit")} onclick={() => ((wsMenu = false), onmanage(w))}>
              <Pencil size={12} />
            </button>
          </div>
        {/each}
        <button class="ws-new" onclick={() => ((wsMenu = false), onmanage(null))}><Plus size={14} />{t("New workspace")}</button>
      </div>
    {/if}
  </div>

  {#if collapsed}
    <button class="search-ico" title={t("Search (Ctrl K)")} onclick={onsearch}>
      <Search size={16} color="var(--ink2)" />
    </button>
  {:else}
    <button class="search" onclick={onsearch}>
      <Search size={15} color="var(--lichen)" />
      <span>{t("Search or open…")}</span>
      <kbd>Ctrl K</kbd>
    </button>
  {/if}

  <div class="list">
    {#if recentServers.length && !collapsed}
      <div class="grp static"><Clock size={12} /><span>{t("Recent")}</span></div>
      {#each recentServers as s (`recent-${s.id}`)}{@render row(s)}{/each}
    {/if}

    {#each groups as [group, items] (group)}
      {@const isFolded = folded.has(group) && !collapsed}
      <button class="grp" class:folded={isFolded} title={group} onclick={() => toggle(group)}>
        <ChevronRight size={12} class="chev" />
        <span class="gname">{group}</span>
        {#if isFolded && groupHasActivity(items)}<span class="dot small"></span>{/if}
        <span class="count">{items.length}</span>
      </button>
      {#if !isFolded}
        {#each items as s (s.id)}{@render row(s)}{/each}
      {/if}
    {:else}
      <p class="empty">{t("No connections yet. Add your first one.")}</p>
      <button class="empty link" onclick={onimport}>{t("Import existing connections…")}</button>
    {/each}
  </div>

  <div class="foot">
    <button class="btn pri" title={t("New connection")} onclick={onnew}>
      <Plus size={16} /><span class="txt">{t("New")}</span>
    </button>
    <button class="btn gear" title="Backups" onclick={onbackups}><ArchiveRestore size={16} color="var(--ink2)" /></button>
    <button class="btn gear" title={t("Settings")} onclick={onsettings}><Settings size={16} color="var(--ink2)" /></button>
  </div>
</aside>

<style>
  .side {
    background: var(--rail);
    border-right: 1px solid var(--mist);
    display: flex;
    flex-direction: column;
    padding: 14px 10px 10px;
    min-height: 0;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 8px 14px;
  }
  .mark {
    width: 28px;
    height: 28px;
    border-radius: 8px;
    background: var(--pine);
    display: grid;
    place-items: center;
    flex: none;
  }
  .brand b {
    font-size: 15px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }
  .fold {
    margin-left: auto;
    padding: 4px;
    border-radius: 6px;
    display: grid;
  }
  .fold:hover {
    background: var(--mist2);
  }
  /* Workspace switcher */
  .ws-wrap {
    position: relative;
    margin-bottom: 10px;
  }
  .ws {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    padding: 6px 8px;
    border-radius: 9px;
    background: var(--ws-tint);
    border: 1px solid color-mix(in srgb, var(--ws) 25%, transparent);
    text-align: left;
  }
  .ws:hover {
    border-color: var(--ws);
  }
  .ws-dot {
    width: 22px;
    height: 22px;
    border-radius: 6px;
    background: var(--ws);
    color: #fff;
    font-weight: 700;
    font-size: 11.5px;
    display: grid;
    place-items: center;
    flex: none;
  }
  .ws-dot.small {
    width: 20px;
    height: 20px;
    font-size: 11px;
  }
  .ws-name {
    flex: 1;
    min-width: 0;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ws-menu {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    min-width: 230px;
    z-index: 25;
    background: var(--paper);
    border: 1px solid var(--mist);
    border-radius: 10px;
    padding: 5px;
    box-shadow: var(--shadow-md);
  }
  .ws-item {
    display: flex;
    align-items: center;
    border-radius: 7px;
  }
  .ws-item:hover,
  .ws-item.on {
    background: var(--mist2);
  }
  .ws-pick {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 6px 8px;
    min-width: 0;
    text-align: left;
  }
  .ws-label {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ws-count {
    font-size: 11px;
    color: var(--lichen);
  }
  .ws-pick kbd {
    font: 10px var(--mono);
    color: var(--lichen);
  }
  .ws-edit {
    padding: 6px;
    border-radius: 6px;
    color: var(--lichen);
    display: grid;
    opacity: 0;
  }
  .ws-item:hover .ws-edit {
    opacity: 1;
  }
  .ws-edit:hover {
    color: var(--granite);
  }
  .ws-new {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 7px 8px;
    margin-top: 3px;
    border-top: 1px solid var(--mist2);
    color: var(--ink2);
    border-radius: 0 0 7px 7px;
  }
  .ws-new:hover {
    color: var(--pine);
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--paper);
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 7px 10px;
    margin-bottom: 12px;
    color: var(--lichen);
    text-align: left;
  }
  .search:hover {
    border-color: var(--line-strong);
  }
  .search span {
    flex: 1;
  }
  .search kbd {
    font: 10.5px var(--mono);
    background: var(--mist2);
    border-radius: 4px;
    padding: 1px 5px;
    color: var(--ink2);
  }
  .search-ico {
    width: 44px;
    height: 36px;
    margin: 0 auto 14px;
    border-radius: 8px;
    border: 1px solid var(--mist);
    background: var(--paper);
    display: grid;
    place-items: center;
  }
  .list {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
  }

  /* Group headers: click to fold. */
  .grp {
    display: flex;
    align-items: center;
    gap: 5px;
    width: 100%;
    padding: 5px 8px 4px 4px;
    margin-top: 10px;
    border-radius: 6px;
    font-size: 11px;
    font-weight: 600;
    color: var(--lichen);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    text-align: left;
  }
  .grp:first-child {
    margin-top: 0;
  }
  button.grp:hover {
    color: var(--ink2);
    background: var(--glass);
  }
  .grp.static {
    padding-left: 8px;
  }
  .grp :global(.chev) {
    transition: transform 0.15s;
    transform: rotate(90deg);
    flex: none;
  }
  .grp.folded :global(.chev) {
    transform: rotate(0deg);
  }
  .gname {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .count {
    font-weight: 500;
  }

  /* One compact line per connection. */
  .srv {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 8px;
    border-radius: 7px;
  }
  .srv:hover {
    background: var(--glass);
  }
  .srv.on {
    background: var(--paper);
    box-shadow: var(--shadow-sm), 0 0 0 1px var(--mist);
  }
  .ic {
    display: grid;
    flex: none;
  }
  .name {
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 0 1 auto;
    min-width: 0;
  }
  .host {
    flex: 1 1 0;
    min-width: 0;
    font-size: 10.5px;
    color: var(--lichen);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: right;
  }
  .edit {
    display: none;
    padding: 3px;
    border-radius: 4px;
    flex: none;
  }
  .srv:hover .edit {
    display: grid;
  }
  .srv:hover .host {
    display: none;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--pine);
    flex: none;
  }
  .dot.small {
    width: 6px;
    height: 6px;
  }
  .dot.off {
    background: transparent;
    border: 1.5px solid var(--control);
  }
  .dot.busy {
    background: var(--amber);
    border: 0;
    animation: pulse 1s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
  .empty {
    color: var(--lichen);
    padding: 8px;
    font-size: 12.5px;
  }
  .empty.link {
    padding-top: 0;
    text-align: left;
    color: var(--pine);
  }
  .empty.link:hover {
    text-decoration: underline;
  }
  .foot {
    display: flex;
    gap: 8px;
    padding-top: 10px;
  }
  .foot .btn {
    flex: 1;
    min-width: 0;
    justify-content: center;
  }
  .gear {
    flex: none !important;
    padding: 7px 9px;
  }

  /* Collapsed icon rail */
  .rail {
    padding: 14px 8px 10px;
  }
  .rail .brand {
    justify-content: center;
    padding: 4px 0 14px;
  }
  .rail .name,
  .rail .host,
  .rail .edit,
  .rail .txt,
  .rail .empty,
  .rail .grp :global(.chev),
  .rail .gname,
  .rail .count {
    display: none !important;
  }
  .rail .ws {
    justify-content: center;
    padding: 6px 0;
  }
  .rail .ws-name,
  .rail .ws > :global(svg) {
    display: none;
  }
  .rail .ws-menu {
    left: 0;
  }
  .rail .grp {
    height: 1px;
    padding: 0;
    margin: 8px 10px;
    background: var(--line-strong);
    pointer-events: none;
  }
  .rail .grp:first-child {
    display: none;
  }
  .rail .srv {
    justify-content: center;
    position: relative;
    height: 40px;
    padding: 0;
  }
  .rail .srv .ic {
    width: 30px;
    height: 30px;
    place-items: center;
    border-radius: 7px;
    background: var(--paper);
    border: 1px solid var(--mist);
  }
  .rail .srv .dot {
    position: absolute;
    right: 8px;
    bottom: 6px;
    box-shadow: 0 0 0 2px var(--rail);
  }
  .rail .foot {
    flex-direction: column;
  }
  .rail .foot .btn {
    padding: 8px 0;
    justify-content: center;
  }
</style>
