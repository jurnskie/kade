<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { ArrowRight, FolderSync, FolderUp, FolderDown, LoaderCircle, TriangleAlert, Search, Plus, RefreshCw, Minus, FileMinus, CircleSlash } from "@lucide/svelte";
  import { api, errorMessage, type LocalLaravel, type SyncChange, type SyncDirection, type SyncPreview, type SyncPreviewItem } from "$lib/api";
  import { t, tn } from "$lib/i18n.svelte";
  import { formatSize } from "$lib/format";
  import Modal from "./Modal.svelte";
  import { radioGroup } from "./connect/radioGroup";

  let {
    sessionId,
    serverName,
    direction: initialDirection,
    local: initialLocal,
    remote: initialRemote,
    pullWarning = null,
    onclose,
  }: {
    sessionId: string;
    serverName: string;
    /** The pane the action came from: a local folder syncs to the server. */
    direction: SyncDirection;
    /** Absolute folders; the destination one can still be edited here. */
    local: string;
    remote: string;
    /** From rsyncSupport: shown before a sync from the server. */
    pullWarning?: string | null;
    onclose: () => void;
  } = $props();

  /** The backend drops a preview after 30 minutes; ask again a little earlier. */
  const PREVIEW_TTL = 29 * 60_000;
  /** A dry run is usually quick; only offer Cancel when it isn't. */
  const SLOW_AFTER = 2000;
  const ROW = 26;
  const OVERSCAN = 6;
  const LIST_H = 200;

  /* svelte-ignore state_referenced_locally */
  let direction = $state(initialDirection);
  /* svelte-ignore state_referenced_locally */
  let local = $state(initialLocal);
  /* svelte-ignore state_referenced_locally */
  let remote = $state(initialRemote);
  let del = $state(false);
  let checksum = $state(false);
  let excludes = $state(".DS_Store");

  let running = $state(false);
  let slow = $state(false);
  let starting = $state(false);
  let error = $state<string | null>(null);
  let preview = $state<SyncPreview | null>(null);
  let previewedAt = 0;
  // A preview that can no longer be started (expired, or the start was refused).
  let stale = $state(false);
  let cancelled = false;
  let previewId: string | null = null;
  let slowTimer: ReturnType<typeof setTimeout> | undefined;
  let sourceNote = $state<string | null>(null);

  let filter = $state<SyncChange | "all">("all");
  let query = $state("");
  let scrollTop = $state(0);

  const toServer = $derived(direction === "to_server");
  const patterns = $derived(
    excludes
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean),
  );
  const canPreview = $derived(!running && !starting && local.trim() !== "" && remote.trim() !== "");

  const matches = $derived.by(() => {
    if (!preview) return [] as SyncPreviewItem[];
    const q = query.trim().toLowerCase();
    return preview.items.filter((i) => (filter === "all" || i.change === filter) && (!q || i.path.toLowerCase().includes(q)));
  });
  const first = $derived(Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN));
  const last = $derived(Math.min(matches.length, Math.ceil((scrollTop + LIST_H) / ROW) + OVERSCAN));
  const windowed = $derived(matches.slice(first, last));

  /** Changing anything after a preview makes that preview a different sync. */
  function dirty() {
    sourceNote = null;
    if (preview || stale || error) {
      preview = null;
      stale = false;
      error = null;
      filter = "all";
      query = "";
      scrollTop = 0;
    }
  }

  function setDirection(d: SyncDirection) {
    if (d === direction) return;
    direction = d;
    sourceNote = null;
    dirty();
    if (d === "from_server") void checkRemoteSource();
  }

  const parentOf = (p: string) => p.replace(/\/+$/, "").replace(/\/[^/]*$/, "") || "/";

  /**
   * A sync from the server often starts from a local folder, so the server
   * path is only a guess (the other pane's folder plus the local name). When
   * that folder isn't there, fall back to the folder the guess was made in.
   */
  async function checkRemoteSource() {
    const guess = remote.trim();
    if (!guess.startsWith("/") || guess === "/") return;
    try {
      await api.remoteList(sessionId, guess);
    } catch {
      if (direction !== "from_server" || remote.trim() !== guess) return;
      const base = parentOf(guess);
      try {
        await api.remoteList(sessionId, base);
      } catch {
        return;
      }
      if (direction !== "from_server" || remote.trim() !== guess) return;
      remote = base;
      sourceNote = t("{path} doesn't exist on the server, so its parent folder is filled in. Change “From” if you meant another folder.", { path: guess });
    }
  }

  onMount(() => {
    if (initialDirection === "from_server") void checkRemoteSource();
    // Only a Laravel project gets shortcuts; anywhere else this stays quiet.
    api.localLaravel(initialLocal.trim()).then((p) => (project = p)).catch(() => {});
  });

  /** Folders of a Laravel project, relative to its root. */
  const SHORTCUTS = [
    { id: "content", rel: "content", label: () => t("Content") },
    { id: "assets", rel: "public/assets", label: () => t("Assets") },
    { id: "storage", rel: "storage/app", label: () => "storage/app" },
  ] as const;
  type Shortcut = (typeof SHORTCUTS)[number];

  let project = $state<LocalLaravel | null>(null);
  let shortcutBusy = $state(false);
  let activeShortcut = $state<string | null>(null);
  let shortcutNotes = $state<string[]>([]);
  let symlinkWhy = $state(false);
  // storage/app is only worth offering when it is there; the others show why they're disabled.
  const shortcuts = $derived(project ? SHORTCUTS.filter((s) => s.id !== "storage" || project!.storage) : []);

  const joinPath = (root: string, rel: string) => `${root.replace(/\/+$/, "")}/${rel}`;

  /** "current/public/assets → shared/public/assets": the shared leading folders are left out. */
  function changed(from: string, to: string) {
    const a = from.split("/");
    const b = to.split("/");
    let i = 0;
    while (i < a.length - 1 && i < b.length - 1 && a[i] === b[i]) i++;
    return `${a.slice(i).join("/")} → ${b.slice(i).join("/")}`;
  }

  /**
   * Fills both folders for a project folder. Symlinks are resolved first, because
   * rsync 3.4+ refuses to write into a symlinked destination (Deployer's
   * `current/public/assets` is one).
   */
  async function applyShortcut(s: Shortcut) {
    if (!project || shortcutBusy || running) return;
    shortcutBusy = true;
    const notes: string[] = [];
    try {
      const localWanted = joinPath(project.root, s.rel);
      const typedRemote = remote.trim();
      const found = await api.remoteLaravelRoot(sessionId, typedRemote);
      let remoteWanted = joinPath(found ?? typedRemote, s.rel);
      let remoteMissing = false;
      if (!found) {
        // No project on the server: the folder may still be there, otherwise its nearest existing parent.
        let at = remoteWanted;
        for (;;) {
          try {
            await api.remoteList(sessionId, at);
            break;
          } catch {
            if (at === "/") break;
            at = parentOf(at);
          }
        }
        if (at === remoteWanted) {
          notes.push(t("No Laravel project found on the server, so {path} is used. Check that it's the right folder.", { path: at }));
        } else {
          notes.push(t("No Laravel project found on the server, and {path} doesn't exist there. Its nearest existing folder, {parent}, is filled in.", { path: remoteWanted, parent: at }));
          remoteWanted = at;
        }
      }
      const localReal = await api.localRealpath(localWanted);
      let remoteReal = remoteWanted;
      try {
        remoteReal = await api.remoteRealpath(sessionId, remoteWanted);
      } catch {
        remoteMissing = true;
      }
      if (localReal !== localWanted) notes.push(t("Symlink resolved on this computer: {change}", { change: changed(localWanted, localReal) }));
      if (remoteReal !== remoteWanted) notes.push(t("Symlink resolved on the server: {change}", { change: changed(remoteWanted, remoteReal) }));
      const resolved = localReal !== localWanted || remoteReal !== remoteWanted;
      dirty();
      local = localReal;
      remote = remoteReal;
      activeShortcut = s.id;
      shortcutNotes = notes;
      symlinkWhy = resolved;
      // A folder that isn't on the server yet only matters when it is the source.
      if (remoteMissing && direction === "from_server") void checkRemoteSource();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      shortcutBusy = false;
    }
  }

  /** Typing in a folder field means the shortcut no longer describes it. */
  function pathEdited() {
    activeShortcut = null;
    shortcutNotes = [];
    symlinkWhy = false;
    dirty();
  }

  /** Both folders must be absolute paths; the server reports the rest (missing, not a folder). */
  function pathProblem(): string | null {
    for (const [name, p] of [[t("From"), toServer ? local : remote], [t("To"), toServer ? remote : local]] as const) {
      if (!p.trim().startsWith("/")) return t("“{name}” must be an absolute path, starting with /", { name });
    }
    return null;
  }

  async function runPreview() {
    if (!canPreview) return;
    const problem = pathProblem();
    if (problem) {
      preview = null;
      stale = false;
      error = problem;
      return;
    }
    const id = crypto.randomUUID();
    previewId = id;
    cancelled = false;
    running = true;
    slow = false;
    error = null;
    preview = null;
    stale = false;
    filter = "all";
    query = "";
    scrollTop = 0;
    slowTimer = setTimeout(() => (slow = true), SLOW_AFTER);
    try {
      const p = await api.syncPreview(sessionId, id, { direction, local: local.trim(), remote: remote.trim(), delete: del, checksum, excludes: patterns });
      if (cancelled) return;
      preview = p;
      previewedAt = Date.now();
    } catch (e) {
      if (!cancelled) error = errorMessage(e);
    } finally {
      clearTimeout(slowTimer);
      running = false;
      slow = false;
      previewId = null;
    }
  }

  function cancelPreview() {
    cancelled = true;
    if (previewId) api.syncPreviewCancel(previewId).catch(() => {});
    running = false;
    slow = false;
    clearTimeout(slowTimer);
  }

  async function start() {
    if (!preview || starting) return;
    if (Date.now() - previewedAt > PREVIEW_TTL) {
      stale = true;
      return;
    }
    starting = true;
    error = null;
    try {
      await api.syncStart(preview.id);
      onclose();
    } catch (e) {
      // The only likely cause is a preview the backend no longer has; making a new one is the way out.
      error = errorMessage(e);
      stale = true;
    } finally {
      starting = false;
    }
  }

  // Closing mid-preview must not leave the dry run going.
  onDestroy(() => {
    clearTimeout(slowTimer);
    if (running && previewId) api.syncPreviewCancel(previewId).catch(() => {});
  });

  function onclosing() {
    if (running) cancelPreview();
    onclose();
  }

  const changeLabel = $derived<Record<SyncChange, string>>({
    new: t("New"),
    updated: t("Updated"),
    deleted: t("Deleted"),
    skipped: t("Skipped"),
  });

  const counts = $derived(
    preview
      ? ({ all: preview.items.length, new: preview.new_files, updated: preview.updated_files, deleted: preview.deleted, skipped: preview.skipped } as const)
      : null,
  );

</script>

<Modal width={660} labelledby="sync-title" onclose={onclosing}>
  {#snippet header(close)}
    <div class="sh">
      <FolderSync size={18} color="var(--pine)" />
      <div>
        <h1 id="sync-title">{t("Sync folder")}</h1>
        <p>{t("Make one folder match another. Kade shows what would change first.")}</p>
      </div>
      {@render close()}
    </div>
  {/snippet}

  <div class="body">
    <div class="dir" role="radiogroup" aria-label={t("Direction")} use:radioGroup>
      <button class:on={toServer} role="radio" aria-checked={toServer} tabindex={toServer ? 0 : -1} disabled={running} onclick={() => setDirection("to_server")}>
        <FolderUp size={16} color={toServer ? "var(--pine)" : "var(--ink2)"} />
        <b>{t("Sync to {name}", { name: serverName })}</b>
      </button>
      <button class:on={!toServer} role="radio" aria-checked={!toServer} tabindex={!toServer ? 0 : -1} disabled={running} onclick={() => setDirection("from_server")}>
        <FolderDown size={16} color={!toServer ? "var(--pine)" : "var(--ink2)"} />
        <b>{t("Sync from {name}", { name: serverName })}</b>
      </button>
    </div>

    {#if project}
      <div class="shortcuts" role="group" aria-label={t("Laravel folders")}>
        <span class="lbl">{t("Laravel folders")}</span>
        {#each shortcuts as s (s.id)}
          {@const exists = s.id === "content" ? project.content : s.id === "assets" ? project.assets : project.storage}
          <button
            class="chip"
            class:on={activeShortcut === s.id}
            aria-pressed={activeShortcut === s.id}
            disabled={!exists || running || shortcutBusy}
            title={exists ? t("Use {path} on both sides", { path: s.rel }) : t("{path} doesn't exist in this project on this computer", { path: s.rel })}
            onclick={() => applyShortcut(s)}
          >
            {s.label()}
          </button>
        {/each}
        {#if shortcutBusy}<LoaderCircle size={14} class="spin" color="var(--lichen)" />{/if}
      </div>
    {/if}

    <div class="paths">
      <label class="side">
        <span class="lbl">{t("From")}</span>
        {#if toServer}
          <input class="mono" bind:value={local} oninput={pathEdited} spellcheck="false" disabled={running} />
        {:else}
          <input class="mono" bind:value={remote} oninput={pathEdited} spellcheck="false" disabled={running} />
        {/if}
        <small>{toServer ? t("This computer") : serverName}</small>
      </label>
      <div class="arrow"><ArrowRight size={16} color="var(--lichen)" /></div>
      <label class="side">
        <span class="lbl">{t("To")}</span>
        {#if toServer}
          <input class="mono" bind:value={remote} oninput={pathEdited} spellcheck="false" disabled={running} />
        {:else}
          <input class="mono" bind:value={local} oninput={pathEdited} spellcheck="false" disabled={running} />
        {/if}
        <small>{toServer ? serverName : t("This computer")}</small>
      </label>
    </div>

    {#if shortcutNotes.length || symlinkWhy}
      <div class="notes">
        {#each shortcutNotes as n, i (i)}
          <p class="note"><TriangleAlert size={14} />{n}</p>
        {/each}
        {#if symlinkWhy}<p class="note quiet">{t("rsync can't write into a symlinked folder, so the real folder is used.")}</p>{/if}
      </div>
    {/if}

    {#if sourceNote}
      <p class="note"><TriangleAlert size={14} />{sourceNote}</p>
    {/if}

    {#if !toServer && pullWarning}
      <p class="note warn"><TriangleAlert size={14} />{pullWarning}</p>
    {/if}

    <div class="opts">
      <label class="check">
        <input type="checkbox" bind:checked={del} onchange={dirty} disabled={running} />
        <span>
          {t("Also delete files that only exist at the destination")}
          <small class:warn={del}>{t("Removes them from the destination. They go to Backups, so you can restore them.")}</small>
        </span>
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={checksum} onchange={dirty} disabled={running} />
        <span>
          {t("Compare file contents")}
          <small>{t("Slower, but doesn't rely on size and modification time.")}</small>
        </span>
      </label>
      <label class="ex">
        <span class="lbl">{t("Skip these (one pattern per line)")}</span>
        <textarea class="mono" rows="2" bind:value={excludes} oninput={dirty} spellcheck="false" placeholder="node_modules&#10;*.log" disabled={running}></textarea>
      </label>
    </div>

    {#if error}<div class="err"><TriangleAlert size={14} />{error}</div>{/if}

    {#if running}
      <div class="state">
        <LoaderCircle size={16} class="spin" color="var(--lichen)" />
        <span>{t("Comparing the folders…")}</span>
        {#if slow}<button class="btn" onclick={cancelPreview}>{t("Cancel")}</button>{/if}
      </div>
    {:else if preview}
      {#if preview.nothing_to_do}
        <div class="state done">
          <FolderSync size={16} color="var(--pine)" />
          <span>{t("Nothing to do: the folders already match.")}</span>
        </div>
      {:else if counts}
        <div class="sum">
          <div class="chips" role="group" aria-label={t("Show")}>
            {#each [["all", t("All")], ["new", changeLabel.new], ["updated", changeLabel.updated], ["deleted", changeLabel.deleted], ["skipped", changeLabel.skipped]] as [id, name] (id)}
              {@const n = id === "all" ? counts.all : counts[id as SyncChange]}
              {#if id === "all" || n > 0}
                <button class="chip" class:on={filter === id} aria-pressed={filter === id} onclick={() => ((filter = id as typeof filter), (scrollTop = 0))}>
                  {name} <b>{n}</b>
                </button>
              {/if}
            {/each}
          </div>
          <span class="bytes">
            {#if preview.new_files}{t("{size} new", { size: formatSize(preview.new_bytes) })}{/if}
            {#if preview.new_files && preview.updated_files} · {/if}
            {#if preview.updated_files}{t("{size} updated", { size: formatSize(preview.updated_bytes) })}{/if}
          </span>
        </div>

        {#if preview.deleted > 0}
          <p class="note warn">
            <TriangleAlert size={14} />
            {tn(preview.deleted, "{n} item at the destination will be deleted. They go to Backups.", "{n} items at the destination will be deleted. They go to Backups.")}
          </p>
        {/if}
        {#if preview.skipped > 0}
          <p class="note"><CircleSlash size={14} />{t("Skipped files are symlinks at the destination. Use a normal upload for those.")}</p>
        {/if}

        <label class="find">
          <Search size={14} color="var(--lichen)" />
          <input bind:value={query} oninput={() => (scrollTop = 0)} placeholder={t("Filter by name")} aria-label={t("Filter by name")} spellcheck="false" />
        </label>
        <div class="changes" style:height="{LIST_H}px" onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)} role="list" tabindex="-1">
          {#if matches.length === 0}
            <div class="none">{t("No matches")}</div>
          {:else}
            <div style:height="{matches.length * ROW}px" style:padding-top="{first * ROW}px" style:box-sizing="border-box">
              {#each windowed as item, i (first + i)}
                <div class="row {item.change}" role="listitem" title={item.path} style:height="{ROW}px">
                  <span class="kind" title={changeLabel[item.change]}>
                    {#if item.change === "new"}<Plus size={13} />
                    {:else if item.change === "updated"}<RefreshCw size={12} />
                    {:else if item.change === "deleted"}<FileMinus size={13} />
                    {:else}<Minus size={13} />{/if}
                  </span>
                  <span class="p mono">{item.path}{item.is_dir && !item.path.endsWith("/") ? "/" : ""}</span>
                  <span class="sz mono">{item.is_dir ? "" : item.size == null ? "—" : formatSize(item.size)}</span>
                </div>
              {/each}
            </div>
          {/if}
        </div>
        {#if preview.truncated}
          <p class="note">
            <TriangleAlert size={14} />
            {t("The list shows the first {n} changes; the totals above count everything.", { n: preview.items.length })}
          </p>
        {/if}
        <p class="note quiet">{t("Files replaced by a sync get your user's default group on the server.")}</p>
      {/if}
    {/if}

    {#if stale}
      <p class="note warn"><TriangleAlert size={14} />{t("This preview is out of date. Preview again before syncing.")}</p>
    {/if}
  </div>

  <div class="foot">
    <button class="btn ghost" onclick={onclosing}>{t("Cancel")}</button>
    {#if preview && !preview.nothing_to_do && !stale}
      <button class="btn" onclick={runPreview} disabled={!canPreview}>{t("Preview again")}</button>
      <button class="btn pri" onclick={start} disabled={starting}>
        {#if starting}<LoaderCircle size={14} class="spin" />{:else}<FolderSync size={14} />{/if}
        {t("Start sync")}
      </button>
    {:else}
      <button class="btn pri" onclick={runPreview} disabled={!canPreview}>
        {#if running}<LoaderCircle size={14} class="spin" />{/if}
        {stale || preview ? t("Preview again") : t("Preview")}
      </button>
    {/if}
  </div>
</Modal>

<style>
  .sh {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 18px 22px 14px;
    border-bottom: 1px solid var(--mist2);
  }
  .sh :global(svg) {
    margin-top: 3px;
  }
  h1 {
    font-size: 18px;
    font-weight: 600;
  }
  .sh p {
    color: var(--lichen);
    margin-top: 2px;
  }
  .body {
    padding: 16px 22px 8px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .dir {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }
  .dir button {
    display: flex;
    align-items: center;
    gap: 8px;
    border: 1px solid var(--mist);
    border-radius: 10px;
    padding: 9px 12px;
    text-align: left;
  }
  .dir button b {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dir button.on {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
  }
  .dir button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
  .paths {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 20px minmax(0, 1fr);
    gap: 8px;
    align-items: start;
  }
  .side {
    display: flex;
    flex-direction: column;
    gap: 5px;
    min-width: 0;
  }
  .side small {
    color: var(--lichen);
    font-size: 11.5px;
  }
  .shortcuts {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .shortcuts .lbl {
    margin-right: 4px;
  }
  .notes {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .arrow {
    height: 34px;
    margin-top: 16px;
    display: grid;
    place-items: center;
  }
  input:not([type="checkbox"]),
  textarea {
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 0 10px;
    background: var(--paper);
    outline: 0;
    width: 100%;
    font-size: 12px;
  }
  input:not([type="checkbox"]) {
    height: 34px;
  }
  textarea {
    padding: 8px 10px;
    resize: vertical;
    min-height: 54px;
    max-height: 140px;
    line-height: 1.5;
  }
  input:not([type="checkbox"]):focus,
  textarea:focus {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
  }
  input:disabled,
  textarea:disabled {
    opacity: 0.6;
  }
  .opts {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .check {
    display: flex;
    align-items: flex-start;
    gap: 9px;
    color: var(--granite);
  }
  .check input {
    accent-color: var(--pine);
    margin-top: 3px;
  }
  .check small.warn {
    color: var(--amber-ink);
  }
  .check small {
    display: block;
    color: var(--lichen);
    font-size: 11.5px;
    margin-top: 1px;
  }
  .ex {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .err {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--danger-t);
    color: var(--danger);
    font-size: 12.5px;
  }
  .err :global(svg) {
    margin-top: 2px;
    flex: none;
  }
  .state {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px;
    border-radius: 10px;
    background: var(--snow);
    border: 1px solid var(--mist2);
    color: var(--ink2);
  }
  .state .btn {
    margin-left: auto;
  }
  .state.done {
    background: var(--pine-t);
    border-color: transparent;
    color: var(--granite);
  }
  .sum {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .chips {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .chip {
    border: 1px solid var(--mist);
    border-radius: 99px;
    padding: 3px 11px;
    font-size: 12px;
    color: var(--ink2);
  }
  .chip b {
    font-weight: 600;
    margin-left: 3px;
    color: var(--granite);
  }
  .chip:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .chip:hover:not(:disabled) {
    background: var(--mist2);
  }
  .chip.on {
    border-color: var(--pine);
    background: var(--pine-t);
  }
  .bytes {
    margin-left: auto;
    font-size: 12px;
    color: var(--lichen);
  }
  .note {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    font-size: 12px;
    line-height: 1.45;
    color: var(--ink2);
  }
  .note :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .note.warn {
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--amber-t);
    color: var(--amber-ink);
  }
  .note.quiet {
    color: var(--lichen);
  }
  .find {
    position: relative;
    display: block;
  }
  .find :global(svg) {
    position: absolute;
    left: 10px;
    top: 10px;
  }
  .find input {
    padding-left: 30px;
    font-family: var(--sans);
  }
  .changes {
    overflow-y: auto;
    border: 1px solid var(--mist);
    border-radius: 8px;
    background: var(--paper);
    flex: none;
  }
  .none {
    padding: 28px;
    text-align: center;
    color: var(--lichen);
  }
  .row {
    display: grid;
    grid-template-columns: 22px minmax(0, 1fr) auto;
    align-items: center;
    gap: 8px;
    padding: 0 10px;
    font-size: 12px;
  }
  .row:hover {
    background: var(--mist2);
  }
  .kind {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 5px;
  }
  .row.new .kind {
    background: var(--pine-t);
    color: var(--pine);
  }
  .row.updated .kind {
    background: var(--amber-t);
    color: var(--amber);
  }
  .row.deleted .kind {
    background: var(--danger-t);
    color: var(--danger);
  }
  .row.skipped .kind {
    background: var(--mist2);
    color: var(--lichen);
  }
  .row.deleted .p {
    color: var(--danger);
  }
  .row.skipped .p {
    color: var(--lichen);
  }
  .p {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sz {
    color: var(--lichen);
    font-size: 11px;
  }
  .foot {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 22px 16px;
  }
</style>
