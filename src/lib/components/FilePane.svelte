<script lang="ts">
  import type { Component } from "svelte";
  import {
    ArrowLeft,
    ArrowUp,
    RefreshCw,
    Eye,
    EyeOff,
    Folder,
    FolderSymlink,
    File,
    FileCode,
    FileBraces,
    FileLock,
    FileTerminal,
    FileText,
    LoaderCircle,
    TriangleAlert,
    FilePlus,
    FolderPlus,
    FolderOpen,
    PencilLine,
    Trash2,
    Copy,
    ArrowUpFromLine,
    ArrowDownToLine,
    FilePen,
    FolderSync,
  } from "@lucide/svelte";
  import { tick, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { errorMessage, joinPath, type Entry, type FileOps, type Side, type Transaction } from "$lib/api";
  import { drag } from "$lib/drag.svelte";
  import { t, tn } from "$lib/i18n.svelte";
  import { collator, formatDate, formatSize, parentPath } from "$lib/format";
  import { claimMenu } from "$lib/modals";
  import Modal from "./Modal.svelte";

  let {
    label,
    icon: Icon,
    accent = false,
    path = $bindable(),
    home = "",
    load,
    ops,
    showPermissions = false,
    footerNote = "",
    showHiddenDefault = false,
    side,
    sessionId,
    refreshKey = 0,
    sendLabel = "",
    onsend,
    syncLabel = "",
    syncReason = null,
    onsync,
    onmenuopen,
    ondeleted,
    onopenfile,
  }: {
    label: string;
    icon: Component<{ size?: number; color?: string }>;
    accent?: boolean;
    path: string;
    home?: string;
    load: (path: string) => Promise<Entry[]>;
    ops: FileOps;
    showPermissions?: boolean;
    footerNote?: string;
    showHiddenDefault?: boolean;
    side: Side;
    /** The tab this pane belongs to; drags only transfer within one tab. */
    sessionId: string;
    /** Bump to reload, e.g. after a transfer into this folder finished. */
    refreshKey?: number;
    /** "Upload to …" / "Download to local"; empty hides the action. */
    sendLabel?: string;
    onsend?: (paths: string[]) => void;
    /** "Sync this folder to server…" / "…from server…"; empty hides the action. */
    syncLabel?: string;
    /** Why syncing is unavailable (shown as the tooltip, item disabled); null when it works. */
    syncReason?: string | null;
    /** Sync a folder of this pane: the selected one, or the current folder. */
    onsync?: (folder: string) => void;
    /** A context menu opened; the owner can use this to check lazily whether sync works. */
    onmenuopen?: () => void;
    ondeleted?: (tx: Transaction) => void;
    /** Open a file in the user's editor (remote files are synced back on save). */
    onopenfile?: (entry: Entry) => void;
  } = $props();

  let entries = $state<Entry[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  // svelte-ignore state_referenced_locally
  let showHidden = $state(showHiddenDefault);
  $effect(() => {
    showHidden = showHiddenDefault;
  });
  // Mutated in place, so a click only re-renders the rows whose state changed.
  const selected = new SvelteSet<string>();
  let anchor = $state<number | null>(null);
  // The row the arrow keys move from; with Shift the selection spans anchor..cursor.
  let cursor: number | null = null;
  let history: string[] = [];
  let reloadTick = $state(0);
  // The folder `entries` belongs to; a reload of the same folder keeps the selection.
  let loadedPath: string | null = null;

  // Rows have a fixed height (see `td`), so only the visible window is rendered.
  const ROW = 33;
  const OVERSCAN = 8;
  let scrollEl = $state<HTMLDivElement>();
  let scrollTop = $state(0);
  let viewH = $state(0);
  let headH = $state(0);

  const visible = $derived.by(() => {
    const byName = collator();
    return entries
      .filter((e) => showHidden || !e.name.startsWith("."))
      .toSorted((a, b) => Number(b.is_dir) - Number(a.is_dir) || byName.compare(a.name, b.name));
  });

  const first = $derived(Math.max(0, Math.floor((scrollTop - headH) / ROW) - OVERSCAN));
  const last = $derived(Math.min(visible.length, Math.ceil((scrollTop + viewH - headH) / ROW) + OVERSCAN));
  const windowed = $derived(visible.slice(first, last));

  const hiddenCount = $derived(showHidden ? 0 : entries.length - visible.length);

  // Breadcrumb segments; paths under home start with "~".
  const crumbs = $derived.by(() => {
    const inHome = home && (path === home || path.startsWith(home + "/"));
    const base = inHome ? home : "";
    const rest = (inHome ? path.slice(home.length) : path).split("/").filter(Boolean);
    const out: { name: string; path: string; gap?: boolean }[] = [{ name: inHome ? "~" : "/", path: inHome ? home : "/" }];
    let acc = base;
    for (const part of rest) {
      acc = `${acc}/${part}`;
      out.push({ name: part, path: acc });
    }
    return out;
  });

  // Earlier segments collapse into one "…" (middle ones first) until the bar fits;
  // the current folder is never shrunk unless it alone is wider than the bar.
  let crumbEl = $state<HTMLElement>();
  let crumbW = $state(0);
  let collapsed = $state(0);
  const shown = $derived.by(() => {
    // `collapsed` can be stale for a moment after the path changed.
    const n = Math.min(collapsed, crumbs.length - 2);
    if (n <= 0) return crumbs;
    const gap = { name: "…", path: crumbs[n].path, gap: true };
    return [crumbs[0], gap, ...crumbs.slice(n + 1)];
  });
  $effect(() => {
    void crumbs;
    void crumbW;
    const el = crumbEl;
    if (!el) return;
    let cancelled = false;
    (async () => {
      collapsed = 0;
      await tick();
      while (!cancelled && el.scrollWidth > el.clientWidth && collapsed < crumbs.length - 2) {
        collapsed++;
        await tick();
      }
    })();
    return () => (cancelled = true);
  });

  $effect(() => {
    const p = path;
    void reloadTick;
    void refreshKey;
    let cancelled = false;
    loading = true;
    error = null;
    // Don't keep listing (and acting on) the previous folder under the new path.
    if (p !== loadedPath) {
      untrack(() => {
        entries = [];
        selected.clear();
        anchor = cursor = null;
        if (scrollEl) scrollEl.scrollTop = 0;
      });
    }
    load(p)
      .then((list) => {
        if (cancelled) return;
        const samePath = p === loadedPath;
        loadedPath = p;
        entries = list;
        const existing = new Set(list.map((e) => e.path));
        for (const sel of [...selected]) if (!samePath || !existing.has(sel)) selected.delete(sel);
        if (pendingSelect && existing.has(pendingSelect)) selected.add(pendingSelect);
        pendingSelect = null;
        anchor = cursor = null;
      })
      .catch((e) => {
        if (cancelled) return;
        loadedPath = p;
        error = errorMessage(e);
      })
      .finally(() => !cancelled && (loading = false));
    return () => (cancelled = true);
  });

  function go(next: string) {
    if (next === path) return;
    history.push(path);
    path = next;
  }

  function back() {
    const prev = history.pop();
    if (prev) path = prev;
  }

  /** Make `path` the only selected item. */
  function selectOnly(path: string) {
    for (const p of selected) if (p !== path) selected.delete(p);
    selected.add(path);
  }

  function click(e: MouseEvent, entry: Entry, index: number) {
    cursor = index;
    if (e.shiftKey && anchor != null) {
      const [a, b] = [Math.min(anchor, index), Math.max(anchor, index)];
      for (const item of visible.slice(a, b + 1)) selected.add(item.path);
    } else if (e.ctrlKey || e.metaKey) {
      if (selected.has(entry.path)) selected.delete(entry.path);
      else selected.add(entry.path);
      anchor = index;
    } else {
      selectOnly(entry.path);
      anchor = index;
    }
  }

  /** Scroll the row at `index` into view, leaving the sticky header uncovered. */
  function reveal(index: number) {
    if (!scrollEl) return;
    const top = index * ROW;
    if (top < scrollEl.scrollTop) scrollEl.scrollTop = top;
    else if (headH + top + ROW > scrollEl.scrollTop + viewH) scrollEl.scrollTop = headH + top + ROW - viewH;
  }

  function moveCursor(e: KeyboardEvent, delta: number) {
    if (!visible.length) return;
    const base = cursor ?? anchor ?? (delta > 0 ? -1 : visible.length);
    const next = Math.min(visible.length - 1, Math.max(0, base + delta));
    if (e.shiftKey) {
      anchor ??= Math.min(Math.max(base, 0), visible.length - 1);
      const [a, b] = [Math.min(anchor, next), Math.max(anchor, next)];
      selected.clear();
      for (const item of visible.slice(a, b + 1)) selected.add(item.path);
    } else {
      selectOnly(visible[next].path);
      anchor = next;
    }
    cursor = next;
    reveal(next);
  }

  function open(entry: Entry) {
    if (entry.is_dir) go(entry.path);
    else onopenfile?.(entry);
  }

  // ---- Context menu & file operations -----------------------------------

  type Dialog =
    | { kind: "new-file" | "new-folder"; value: string; error?: string }
    | { kind: "rename"; entry: Entry; value: string; error?: string }
    | { kind: "delete"; paths: string[]; error?: string };

  let menu = $state<{ x: number; y: number; entry: Entry | null } | null>(null);
  let dialog = $state<Dialog | null>(null);
  let busy = $state(false);
  let pendingSelect: string | null = null;
  let nameInput = $state<HTMLInputElement>();
  let menuEl = $state<HTMLDivElement>();
  const menuOpen = $derived(menu !== null);
  // Only one context menu app-wide (the other pane's closes), and Escape closes it.
  $effect(() => {
    if (menuOpen) return claimMenu(() => (menu = null));
  });

  async function openMenu(e: MouseEvent, entry: Entry | null, index = -1) {
    e.preventDefault();
    e.stopPropagation();
    if (entry && !selected.has(entry.path)) {
      selectOnly(entry.path);
      anchor = index;
    }
    if (!entry) selected.clear();
    const { clientX: x, clientY: y } = e;
    menu = { x, y, entry };
    onmenuopen?.();
    // Keep the menu inside the window: measure it, then shift it left or flip it above the pointer.
    await tick();
    if (!menu || !menuEl) return;
    const { width, height } = menuEl.getBoundingClientRect();
    const edge = 8;
    const fitsBelow = y + height + edge <= window.innerHeight;
    menu = {
      ...menu,
      x: Math.max(edge, Math.min(x, window.innerWidth - width - edge)),
      y: fitsBelow ? y : Math.max(edge, Math.min(y - height, window.innerHeight - height - edge)),
    };
  }

  async function ask(d: Dialog) {
    menu = null;
    dialog = d;
    await tick();
    if (nameInput && d.kind !== "delete") {
      nameInput.focus();
      // Select the name without its extension, like a file manager.
      const dot = d.value.lastIndexOf(".");
      nameInput.setSelectionRange(0, d.kind === "rename" && dot > 0 ? dot : d.value.length);
    }
  }

  function validName(name: string): string | null {
    if (!name.trim()) return t("The name can't be empty");
    if (name.includes("/")) return t("The name can't contain /");
    if (name === "." || name === "..") return t("Invalid name");
    return null;
  }

  async function confirmDialog() {
    if (!dialog || busy) return;
    const d = dialog;
    busy = true;
    try {
      if (d.kind === "delete") {
        const tx = await ops.remove(d.paths);
        if (tx) ondeleted?.(tx);
      } else {
        const name = d.value.trim();
        const invalid = validName(name);
        if (invalid) {
          dialog = { ...d, error: invalid };
          return;
        }
        const target = joinPath(path, name);
        if (d.kind === "new-file") await ops.createFile(target);
        else if (d.kind === "new-folder") await ops.mkdir(target);
        else if (d.kind === "rename" && name !== d.entry.name) await ops.rename(d.entry.path, target);
        pendingSelect = target;
      }
      dialog = null;
      reloadTick++;
    } catch (e) {
      dialog = { ...d, error: errorMessage(e) };
    } finally {
      busy = false;
    }
  }

  function askDelete(paths: string[]) {
    if (paths.length) ask({ kind: "delete", paths });
  }

  async function copyPaths(paths: string[]) {
    menu = null;
    try {
      await navigator.clipboard.writeText(paths.join("\n"));
    } catch (e) {
      error = errorMessage(e);
    }
  }

  function onPaneKey(e: KeyboardEvent) {
    if (dialog || (e.target as HTMLElement).closest("input")) return;
    const sel = visible.filter((v) => selected.has(v.path));
    if (e.key === "Delete" && sel.length) askDelete(sel.map((v) => v.path));
    else if (e.key === "F2" && sel.length === 1) ask({ kind: "rename", entry: sel[0], value: sel[0].name });
    else if (e.key === "Enter" && sel.length === 1) open(sel[0]);
    else if (e.key === "Backspace") go(parentPath(path));
    else if (e.key === "ArrowDown") moveCursor(e, 1);
    else if (e.key === "ArrowUp") moveCursor(e, -1);
    else return;
    e.preventDefault();
  }

  function startDrag(e: PointerEvent, entry: Entry, index: number) {
    drag.arm(e, () => {
      if (!selected.has(entry.path)) {
        selectOnly(entry.path);
        anchor = index;
      }
      return { side, sessionId, paths: visible.filter((v) => selected.has(v.path)).map((v) => v.path) };
    });
  }

  const dropHere = $derived(
    drag.source != null &&
      drag.over?.side === side &&
      drag.over.sessionId === sessionId &&
      !(drag.source.side === side && drag.source.sessionId === sessionId),
  );

  function iconFor(entry: Entry) {
    if (entry.is_dir) return entry.is_symlink ? FolderSymlink : Folder;
    const name = entry.name.toLowerCase();
    if (name.startsWith(".env") || /\.(pem|key|crt)$/.test(name)) return FileLock;
    if (/\.(json|ya?ml|toml|lock)$/.test(name)) return FileBraces;
    if (/\.(sh|bash|zsh|fish)$/.test(name) || name === "artisan" || entry.permissions?.[3] === "x")
      return FileTerminal;
    if (/\.(php|js|ts|svelte|vue|jsx|tsx|rs|go|py|rb|css|html|blade\.php)$/.test(name)) return FileCode;
    if (/\.(md|txt|log)$/.test(name)) return FileText;
    return File;
  }
</script>

<svelte:window onclick={() => (menu = null)} onblur={() => (menu = null)} />

<!-- Focusable so Delete/F2/Enter work on the selection. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<section class="pane" aria-label={label} tabindex="0" onkeydown={onPaneKey}>
  <header class="ph">
    <div class="loc">
      <Icon size={16} color={accent ? "var(--pine)" : "var(--ink2)"} />
      <em class:accent>{label}</em>
    </div>
    <nav class="crumb mono" class:tight={collapsed >= crumbs.length - 2 && collapsed > 0} bind:this={crumbEl} bind:clientWidth={crumbW}>
      {#each shown as c, i (c.path + (c.gap ? "…" : ""))}
        {#if i > 0 && shown[i - 1].name !== "/"}<span class="sep">/</span>{/if}
        <button class:last={i === shown.length - 1} title={c.path} onclick={() => go(c.path)}>{c.name}</button>
      {/each}
    </nav>
    <div class="tools">
      <button title={t("Back")} onclick={back}><ArrowLeft size={15} /></button>
      <button title={t("Up one folder")} onclick={() => go(parentPath(path))}><ArrowUp size={15} /></button>
      <button title={t("Refresh")} onclick={() => reloadTick++}><RefreshCw size={15} /></button>
      <button title={showHidden ? t("Hide hidden files") : t("Show hidden files")} onclick={() => (showHidden = !showHidden)}>
        {#if showHidden}<Eye size={15} />{:else}<EyeOff size={15} />{/if}
      </button>
    </div>
  </header>

  <div class="load" class:on={loading}></div>

  <div
    class="scroll"
    class:drop={dropHere && drag.over?.dir === path}
    role="presentation"
    bind:this={scrollEl}
    bind:clientHeight={viewH}
    onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}
    data-drop-side={side}
    data-drop-session={sessionId}
    data-drop-path={path}
    oncontextmenu={(e) => openMenu(e, null)}
  >
    <table>
      <thead bind:clientHeight={headH}>
        <tr>
          <th>{t("Name")}</th>
          <th class="r">{t("Size")}</th>
          <th class="c-mod">{t("Modified")}</th>
          {#if showPermissions}<th class="c-perm">{t("Permissions")}</th>{/if}
        </tr>
      </thead>
      <tbody>
        {#if first > 0}<tr class="sp" style:height="{first * ROW}px"><td colspan="4"></td></tr>{/if}
        {#each windowed as entry, k (entry.path)}
          {@const i = first + k}
          {@const EntryIcon = iconFor(entry)}
          <tr
            class:sel={selected.has(entry.path)}
            class:drop={dropHere && entry.is_dir && drag.over?.dir === entry.path}
            data-drop-dir={entry.is_dir ? entry.path : undefined}
            onpointerdown={(e) => startDrag(e, entry, i)}
            onclick={(e) => click(e, entry, i)}
            ondblclick={() => open(entry)}
            oncontextmenu={(e) => openMenu(e, entry, i)}
          >
            <td class="n">
              <div>
                <EntryIcon size={16} color={selected.has(entry.path) ? "var(--pine)" : "var(--lichen)"} />
                <span>{entry.name}</span>
              </div>
            </td>
            <td class="s mono">{entry.is_dir ? "—" : formatSize(entry.size)}</td>
            <td class="m c-mod">{formatDate(entry.modified)}</td>
            {#if showPermissions}<td class="p mono c-perm">{entry.permissions ?? ""}</td>{/if}
          </tr>
        {/each}
        {#if last < visible.length}<tr class="sp" style:height="{(visible.length - last) * ROW}px"><td colspan="4"></td></tr>{/if}
      </tbody>
    </table>

    {#if loading && entries.length === 0}
      <div class="state"><LoaderCircle size={18} class="spin" color="var(--lichen)" /></div>
    {:else if error}
      <div class="state err"><TriangleAlert size={16} color="var(--danger)" />{error}</div>
    {:else if visible.length === 0 && hiddenCount > 0}
      <div class="state">
        {t("Only hidden items ({n})", { n: hiddenCount })}
        <button class="btn" onclick={() => (showHidden = true)}><Eye size={14} />{t("Show")}</button>
      </div>
    {:else if visible.length === 0}
      <div class="state">{t("This folder is empty")}</div>
    {/if}
  </div>

  {#if menu}
    {@const sel = visible.filter((v) => selected.has(v.path))}
    <div class="menu" role="menu" bind:this={menuEl} style:left="{menu.x}px" style:top="{menu.y}px">
      {#if menu.entry}
        {#if sendLabel && onsend}
          <button role="menuitem" class="send" onclick={() => ((menu = null), onsend(sel.map((v) => v.path)))}>
            {#if side === "local"}<ArrowUpFromLine size={15} />{:else}<ArrowDownToLine size={15} />{/if}
            {sendLabel}{#if sel.length > 1}&nbsp;({sel.length}){/if}
          </button>
          <hr />
        {/if}
        {#if !menu.entry.is_dir && sel.length === 1 && onopenfile}
          <button role="menuitem" onclick={() => ((menu = null), onopenfile(sel[0]))}>
            <FilePen size={15} />{side === "remote" ? t("Edit in editor") : t("Open in editor")}
          </button>
        {/if}
        {#if menu.entry.is_dir && sel.length === 1}
          <button role="menuitem" onclick={() => ((menu = null), open(sel[0]))}><FolderOpen size={15} />{t("Open")}</button>
        {/if}
        {#if menu.entry.is_dir && sel.length === 1 && syncLabel && onsync}
          <button role="menuitem" disabled={syncReason != null} title={syncReason ?? undefined} onclick={() => ((menu = null), onsync(sel[0].path))}>
            <FolderSync size={15} />{syncLabel}
          </button>
        {/if}
        {#if sel.length === 1}
          <button role="menuitem" onclick={() => ask({ kind: "rename", entry: sel[0], value: sel[0].name })}>
            <PencilLine size={15} />{t("Rename")}<kbd>F2</kbd>
          </button>
        {/if}
        <button role="menuitem" onclick={() => copyPaths(sel.map((v) => v.path))}>
          <Copy size={15} />{sel.length > 1 ? t("Copy {n} paths", { n: sel.length }) : t("Copy path")}
        </button>
        <button role="menuitem" class="danger" onclick={() => askDelete(sel.map((v) => v.path))}>
          <Trash2 size={15} />{sel.length > 1 ? t("Delete {n} items", { n: sel.length }) : t("Delete")}<kbd>Del</kbd>
        </button>
        <hr />
      {/if}
      <button role="menuitem" onclick={() => ask({ kind: "new-file", value: t("new-file.txt") })}>
        <FilePlus size={15} />{t("New file")}
      </button>
      <button role="menuitem" onclick={() => ask({ kind: "new-folder", value: t("new folder") })}>
        <FolderPlus size={15} />{t("New folder")}
      </button>
      {#if !menu.entry && syncLabel && onsync}
        <button role="menuitem" disabled={syncReason != null} title={syncReason ?? undefined} onclick={() => ((menu = null), onsync(path))}>
          <FolderSync size={15} />{syncLabel}
        </button>
      {/if}
      <hr />
      <button role="menuitem" onclick={() => ((menu = null), reloadTick++)}><RefreshCw size={15} />{t("Refresh")}</button>
      <button role="menuitem" onclick={() => ((menu = null), (showHidden = !showHidden))}>
        {#if showHidden}<EyeOff size={15} />{t("Hide hidden")}{:else}<Eye size={15} />{t("Show hidden")}{/if}
      </button>
    </div>
  {/if}

  {#if dialog}
    <Modal width={420} z={41} top="30%" blur={false} pad="18px 20px 16px" gap={10} onclose={() => (dialog = null)}>
      {#if dialog.kind === "delete"}
        <h3>{dialog.paths.length === 1 ? t("Delete?") : t("Delete {n} items?", { n: dialog.paths.length })}</h3>
        {@const paths = dialog.paths}
        {@const hasFolder = entries.some((e) => e.is_dir && paths.includes(e.path))}
        <p>
          {#if paths.length === 1}<span class="mono">{paths[0]}</span><br />{/if}
          {#if side === "local"}
            {#if hasFolder}
              {paths.length === 1
                ? t("This will be deleted, including folder contents. Kade keeps a backup, so you can restore it from Backups.")
                : t("All selected items will be deleted, including folder contents. Kade keeps a backup, so you can restore them from Backups.")}
            {:else}
              {paths.length === 1
                ? t("This will be deleted. Kade keeps a backup, so you can restore it from Backups.")
                : t("All selected items will be deleted. Kade keeps a backup, so you can restore them from Backups.")}
            {/if}
          {:else if hasFolder}
            {paths.length === 1
              ? t("This will be deleted on {server}, including folder contents. Kade keeps a backup, so you can restore it from Backups.", { server: label })
              : t("All selected items will be deleted on {server}, including folder contents. Kade keeps a backup, so you can restore them from Backups.", { server: label })}
          {:else}
            {paths.length === 1
              ? t("This will be deleted on {server}. Kade keeps a backup, so you can restore it from Backups.", { server: label })
              : t("All selected items will be deleted on {server}. Kade keeps a backup, so you can restore them from Backups.", { server: label })}
          {/if}
        </p>
      {:else}
        <h3>{dialog.kind === "new-file" ? t("New file") : dialog.kind === "new-folder" ? t("New folder") : t("Rename")}</h3>
        <input
          class="mono"
          bind:this={nameInput}
          bind:value={dialog.value}
          spellcheck="false"
          onkeydown={(e) => e.key === "Enter" && confirmDialog()}
        />
        <p class="where mono">{t("in {path}", { path })}</p>
      {/if}
      {#if dialog.error}<p class="err">{dialog.error}</p>{/if}
      <div class="da">
        <button class="btn ghost" onclick={() => (dialog = null)}>{t("Cancel")}</button>
        <!-- svelte-ignore a11y_autofocus -->
        <button
          class="btn pri"
          class:del={dialog.kind === "delete"}
          disabled={busy}
          autofocus={dialog.kind === "delete"}
          onclick={confirmDialog}
        >
          {#if busy}<LoaderCircle size={14} class="spin" />{/if}
          {dialog.kind === "delete" ? t("Delete") : dialog.kind === "rename" ? t("Rename") : t("Create")}
        </button>
      </div>
    </Modal>
  {/if}

  <footer class="pf">
    <span>
      {#if selected.size > 0}{t("{n} of {total} selected", { n: selected.size, total: visible.length })}{:else}{tn(visible.length, "{n} item", "{n} items")}{/if}{#if hiddenCount > 0} · {t("{n} hidden", { n: hiddenCount })}{/if}
    </span>
    <span class="mono">{footerNote}</span>
  </footer>
</section>

<style>
  .pane:focus {
    outline: none;
  }
  .pane:focus-within {
    border-color: var(--line-strong);
  }
  .menu {
    position: fixed;
    z-index: 40;
    min-width: 210px;
    width: max-content;
    max-height: calc(100vh - 16px);
    overflow-y: auto;
    white-space: nowrap;
    background: var(--paper);
    border: 1px solid var(--mist);
    border-radius: 10px;
    padding: 5px;
    box-shadow: var(--shadow-md);
    display: flex;
    flex-direction: column;
  }
  .menu button {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 6px 9px;
    border-radius: 6px;
    text-align: left;
    color: var(--granite);
  }
  .menu button:hover:not(:disabled) {
    background: var(--pine-t);
  }
  .menu button:disabled {
    color: var(--faint);
    cursor: not-allowed;
  }
  .menu button.send {
    color: var(--pine);
    font-weight: 500;
  }
  .scroll.drop,
  tr.drop td {
    background: var(--pine-t);
  }
  .scroll.drop {
    box-shadow: inset 0 0 0 2px var(--pine);
  }
  .menu button.danger {
    color: var(--danger);
  }
  .menu button.danger:hover {
    background: var(--danger-t);
  }
  .menu kbd {
    margin-left: auto;
    font: 10.5px var(--mono);
    color: var(--lichen);
  }
  .menu hr {
    border: 0;
    border-top: 1px solid var(--mist2);
    margin: 4px 2px;
  }
  h3 {
    font-size: 15px;
    font-weight: 600;
  }
  p {
    color: var(--ink2);
    line-height: 1.5;
  }
  /* Only paths and file names may break mid-word. */
  p .mono,
  p.where {
    overflow-wrap: anywhere;
  }
  p.where {
    font-size: 11.5px;
    color: var(--lichen);
  }
  p.err {
    color: var(--danger);
    font-size: 12.5px;
  }
  input {
    height: 36px;
    border: 1px solid var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
    border-radius: 8px;
    padding: 0 11px;
    outline: 0;
    font-size: 12.5px;
  }
  .da {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .btn.del {
    background: var(--danger);
    border-color: var(--danger);
  }
  .pane {
    container: pane / inline-size;
    background: var(--paper);
    border: 1px solid var(--mist);
    border-radius: 12px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-height: 0;
    min-width: 0;
  }
  .ph {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 10px 10px 14px;
    border-bottom: 1px solid var(--mist);
  }
  .loc {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }
  .loc em {
    font-style: normal;
    font-weight: 500;
    font-size: 11px;
    color: var(--lichen);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .loc em.accent {
    color: var(--pine);
  }
  .crumb {
    display: flex;
    align-items: center;
    gap: 2px;
    font-size: 12px;
    color: var(--lichen);
    /* Width independent of the content, so collapsing segments can't change what is measured. */
    flex: 1 1 0;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }
  .crumb button {
    padding: 2px 3px;
    border-radius: 4px;
    flex: none;
  }
  .crumb button:hover {
    background: var(--mist2);
    color: var(--granite);
  }
  /* Capped at the bar's width, so the measurement sees the overflow of the segments before it. */
  .crumb button.last {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--granite);
    font-weight: 500;
  }
  /* Nothing left to collapse: the current folder gives way, the rest stays. */
  .crumb.tight button.last {
    flex: 0 1 auto;
    min-width: 0;
  }
  .sep {
    flex: none;
    opacity: 0.6;
  }
  .tools {
    margin-left: auto;
    display: flex;
    gap: 2px;
    flex: none;
    color: var(--ink2);
  }
  .tools button {
    padding: 5px;
    border-radius: 6px;
    display: grid;
  }
  .tools button:hover {
    background: var(--mist2);
  }
  .scroll {
    -webkit-user-select: none;
    user-select: none;
    flex: 1;
    overflow: auto;
    min-height: 0;
    position: relative;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th {
    position: sticky;
    top: 0;
    background: var(--paper);
    font-size: 11px;
    font-weight: 600;
    color: var(--lichen);
    text-align: left;
    padding: 8px 14px 6px;
    border-bottom: 1px solid var(--mist2);
    z-index: 1;
  }
  th.r {
    text-align: right;
  }
  tr.sp td {
    height: auto;
    padding: 0;
    border: 0;
  }
  .load {
    height: 2px;
    flex: none;
    background: linear-gradient(90deg, transparent, var(--pine), transparent) 0 0 / 40% 100% no-repeat;
    opacity: 0;
  }
  .load.on {
    opacity: 1;
    animation: load 1s ease-in-out infinite;
  }
  @keyframes load {
    from {
      background-position-x: -40%;
    }
    to {
      background-position-x: 140%;
    }
  }
  td {
    padding: 0 14px;
    height: 33px;
    border-bottom: 1px solid var(--mist2);
    white-space: nowrap;
  }
  tr:hover td {
    background: var(--zebra);
  }
  td.n {
    width: 100%;
    max-width: 0;
  }
  td.n div {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  td.n span {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  td.m {
    color: var(--lichen);
    font-size: 12px;
  }
  td.s {
    color: var(--ink2);
    font-size: 12px;
    text-align: right;
  }
  td.p {
    font-size: 11.5px;
    color: var(--lichen);
  }
  tr.sel td {
    background: var(--pine-t);
  }
  tr.sel td:first-child {
    box-shadow: inset 3px 0 0 var(--pine);
    font-weight: 600;
  }
  .state {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 40px 20px;
    color: var(--lichen);
    text-align: center;
  }
  .state.err {
    color: var(--danger);
  }
  /* Drop secondary columns and the label as the pane narrows. */
  @container pane (max-width: 560px) {
    .c-perm {
      display: none;
    }
  }
  @container pane (max-width: 420px) {
    .c-mod,
    .loc em {
      display: none;
    }
    .pf .mono {
      display: none;
    }
  }
  .pf {
    padding: 8px 14px;
    font-size: 11.5px;
    color: var(--lichen);
    border-top: 1px solid var(--mist2);
    display: flex;
    justify-content: space-between;
  }
</style>
