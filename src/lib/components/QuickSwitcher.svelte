<script lang="ts">
  import { onMount, tick } from "svelte";
  import { Search, Server, SquareTerminal, FolderUp, Plus, CornerDownLeft } from "@lucide/svelte";
  import type { ServerProfile, Workspace } from "$lib/api";
  import { colorOf, workspaceIdOf } from "$lib/workspaces";
  import { locale, t } from "$lib/i18n.svelte";

  let {
    servers,
    recent,
    workspaces,
    activeWorkspace,
    connected,
    onopen,
    onnew,
    onclose,
  }: {
    servers: ServerProfile[];
    recent: string[];
    workspaces: Workspace[];
    activeWorkspace: string;
    connected: Set<string>;
    onopen: (s: ServerProfile) => void;
    onnew: () => void;
    onclose: () => void;
  } = $props();

  const wsOf = (s: ServerProfile) => workspaces.find((w) => w.id === workspaceIdOf(s));

  let query = $state("");
  let index = $state(0);
  let input = $state<HTMLInputElement>();
  let listEl = $state<HTMLDivElement>();

  onMount(() => input?.focus());

  /** Every word must appear somewhere; names that start with the query rank first. */
  const results = $derived.by(() => {
    const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    const rank = (s: ServerProfile) => {
      const r = recent.indexOf(s.id);
      return r === -1 ? 100 : r;
    };
    return servers
      .filter((s) => {
        const ws = workspaces.find((w) => w.id === workspaceIdOf(s))?.name ?? "";
        const hay = `${s.name} ${s.host} ${s.user} ${s.group} ${ws} ${s.protocol} ${s.remote_path ?? ""}`.toLowerCase();
        return words.every((w) => hay.includes(w));
      })
      .sort((a, b) => {
        const q = words[0] ?? "";
        const pa = q && a.name.toLowerCase().startsWith(q) ? 0 : 1;
        const pb = q && b.name.toLowerCase().startsWith(q) ? 0 : 1;
        const wa = workspaceIdOf(a) === activeWorkspace ? 0 : 1;
        const wb = workspaceIdOf(b) === activeWorkspace ? 0 : 1;
        return pa - pb || wa - wb || rank(a) - rank(b) || a.name.localeCompare(b.name, locale());
      })
      .slice(0, 50);
  });

  $effect(() => {
    void query;
    index = 0;
  });

  async function move(delta: number) {
    if (!results.length) return;
    index = (index + delta + results.length) % results.length;
    await tick();
    listEl?.querySelector(".on")?.scrollIntoView({ block: "nearest" });
  }

  function choose(s: ServerProfile | undefined) {
    if (!s) return;
    onclose();
    onopen(s);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown") (e.preventDefault(), move(1));
    else if (e.key === "ArrowUp") (e.preventDefault(), move(-1));
    else if (e.key === "Enter") (e.preventDefault(), results.length ? choose(results[index]) : (onclose(), onnew()));
    else if (e.key === "Escape") (e.preventDefault(), onclose());
  }
</script>

<div class="scrim" role="presentation" onclick={onclose}></div>
<div class="qs" role="dialog" aria-modal="true" aria-label={t("Quick open")}>
  <label class="field">
    <Search size={17} color="var(--lichen)" />
    <input
      bind:this={input}
      bind:value={query}
      placeholder={t("Name, host, group or folder…")}
      spellcheck="false"
      onkeydown={onKey}
    />
    <kbd>Esc</kbd>
  </label>
  <div class="list" bind:this={listEl}>
    {#each results as s, i (s.id)}
      <button class="item" class:on={i === index} onmousemove={() => (index = i)} onclick={() => choose(s)}>
        <span class="ic">
          {#if s.protocol === "ssh"}<SquareTerminal size={16} />
          {:else if s.protocol === "sftp"}<Server size={16} />
          {:else}<FolderUp size={16} />{/if}
        </span>
        <span class="t">
          <b>{s.name}</b>
          <small class="mono">{s.user}@{s.host}{s.remote_path ? ` · ${s.remote_path}` : ""}</small>
        </span>
        {#if workspaces.length > 1 && wsOf(s)}
          <span class="wsl" style:color={colorOf(wsOf(s)).ink} style:background={colorOf(wsOf(s)).tint}>{wsOf(s)?.name}</span>
        {/if}
        <span class="group">{s.group || t("Other")}</span>
        {#if connected.has(s.id)}<span class="dot" title={t("Connected")}></span>{/if}
        {#if i === index}<CornerDownLeft size={13} color="var(--lichen)" />{/if}
      </button>
    {:else}
      <button class="item on" onclick={() => (onclose(), onnew())}>
        <span class="ic"><Plus size={16} /></span>
        <span class="t"><b>{t("New connection")}</b><small>{t("Nothing found for “{q}”", { q: query })}</small></span>
        <CornerDownLeft size={13} color="var(--lichen)" />
      </button>
    {/each}
  </div>
  <div class="foot">
    <span><kbd>↑</kbd><kbd>↓</kbd> {t("choose")}</span>
    <span><kbd>Enter</kbd> {t("open")}</span>
    <span>{t("{n} of {total}", { n: results.length, total: servers.length })}</span>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    z-index: 30;
  }
  .qs {
    position: fixed;
    left: 50%;
    top: 12vh;
    transform: translateX(-50%);
    width: min(600px, calc(100vw - 32px));
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    background: var(--paper);
    border-radius: 14px;
    box-shadow: var(--shadow-lg), 0 0 0 1px var(--mist);
    z-index: 31;
    overflow: hidden;
  }
  .field {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--mist2);
  }
  .field input {
    flex: 1;
    border: 0;
    outline: 0;
    font-size: 15px;
    background: transparent;
  }
  kbd {
    font: 10.5px var(--mono);
    background: var(--mist2);
    border-radius: 4px;
    padding: 1px 5px;
    color: var(--ink2);
  }
  .list {
    overflow-y: auto;
    padding: 6px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border-radius: 8px;
    text-align: left;
    color: var(--ink2);
  }
  .item.on {
    background: var(--pine-t);
    color: var(--pine);
  }
  .ic {
    display: grid;
    flex: none;
  }
  .t {
    flex: 1;
    min-width: 0;
  }
  .t b {
    display: block;
    font-weight: 600;
    color: var(--granite);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .t small {
    display: block;
    font-size: 11.5px;
    color: var(--lichen);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .group {
    font-size: 11px;
    color: var(--lichen);
    background: var(--mist2);
    border-radius: 4px;
    padding: 2px 6px;
    flex: none;
    max-width: 30%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .wsl {
    font-size: 11px;
    font-weight: 600;
    border-radius: 4px;
    padding: 2px 6px;
    flex: none;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--pine);
    flex: none;
  }
  .foot {
    display: flex;
    gap: 14px;
    padding: 8px 14px;
    border-top: 1px solid var(--mist2);
    font-size: 11.5px;
    color: var(--lichen);
  }
  .foot span:last-child {
    margin-left: auto;
  }
  .foot kbd {
    margin-right: 3px;
  }
</style>
