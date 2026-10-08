<script lang="ts">
  import { onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";
  import { RotateCcw } from "@lucide/svelte";
  import { api, errorMessage } from "$lib/api";
  import { t } from "$lib/i18n.svelte";
  import { theme } from "$lib/theme.svelte";
  import { termTheme } from "$lib/termTheme";

  let { sessionId, visible, title }: { sessionId: string; visible: boolean; title: string } = $props();

  let host: HTMLDivElement;
  let term: Terminal;
  let fit: FitAddon;
  let termId: string | null = null;
  let destroyed = false;
  let exited = $state(false);
  let error = $state<string | null>(null);

  async function start() {
    if (destroyed) return;
    exited = false;
    error = null;
    term.reset();
    fit.fit();
    try {
      const id = await api.terminalOpen(sessionId, term.cols, term.rows, (e) => {
        if (destroyed) return;
        if (e.type === "data") term.write(new Uint8Array(e.bytes));
        else {
          exited = true;
          termId = null;
          term.write(`\r\n\x1b[2m[${t("session ended")}]\x1b[0m\r\n`);
        }
      });
      // Unmounted while the shell was starting: nobody is left to close it.
      if (destroyed) {
        api.terminalClose(id).catch(() => {});
        return;
      }
      termId = exited ? null : id;
      if (visible) term.focus();
    } catch (e) {
      if (!destroyed) error = errorMessage(e);
    }
  }

  onMount(() => {
    term = new Terminal({
      fontFamily: '"JetBrains Mono", ui-monospace, monospace',
      fontSize: 13,
      lineHeight: 1.35,
      cursorBlink: true,
      allowProposedApi: false,
      scrollback: 10000,
      theme: termTheme(),
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);

    // The app's own shortcuts (switch workspace, quick switcher, files/terminal)
    // must reach the window; xterm would otherwise swallow them or send them to the shell.
    term.attachCustomKeyEventHandler((e) => {
      const mod = e.ctrlKey || e.metaKey;
      const appKey =
        (mod && !e.shiftKey && !e.altKey && /^[1-9]$/.test(e.key)) ||
        (mod && e.key.toLowerCase() === "k") ||
        (e.ctrlKey && e.key === "`");
      return !appKey;
    });

    term.onData((data) => {
      if (termId) api.terminalWrite(termId, data).catch(() => {});
    });
    term.onResize(({ cols, rows }) => {
      if (termId) api.terminalResize(termId, cols, rows).catch(() => {});
    });

    // Refit when the container changes size; skip while hidden (0×0).
    const ro = new ResizeObserver(() => {
      if (host.clientWidth > 0 && host.clientHeight > 0) fit.fit();
    });
    ro.observe(host);

    document.fonts.ready.then(start);

    return () => {
      destroyed = true;
      ro.disconnect();
      if (termId) api.terminalClose(termId).catch(() => {});
      term.dispose();
    };
  });

  // Recolour open terminals when the palette changes. applyTheme has already
  // updated the DOM by the time this runs, so the tokens read fresh.
  $effect(() => {
    theme.palette;
    if (term) term.options.theme = termTheme();
  });

  $effect(() => {
    if (visible && term) {
      requestAnimationFrame(() => {
        fit.fit();
        term.focus();
      });
    }
  });
</script>

<section class="term">
  <div class="th">
    <span class="st"><i class:off={exited}></i>{title}</span>
    {#if exited || error}
      <button class="re" onclick={start}><RotateCcw size={13} />{t("Restart")}</button>
    {/if}
  </div>
  {#if error}<div class="err">{error}</div>{/if}
  <div class="host" bind:this={host}></div>
</section>

<style>
  .term {
    flex: 1;
    min-height: 0;
    background: var(--term-bg);
    border-radius: 12px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 0 0 1px var(--term-raise);
    -webkit-user-select: text;
    user-select: text;
  }
  .th {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--term-line);
  }
  .st {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 5px 10px;
    border-radius: 6px;
    background: var(--term-chip);
    color: var(--term-15);
    font: 500 12px var(--mono);
  }
  .st i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--term-2);
  }
  .st i.off {
    background: var(--term-8);
  }
  .re {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    border-radius: 6px;
    background: var(--term-raise);
    border: 1px solid var(--term-line2);
    color: var(--term-fg);
    font: 500 12px var(--sans);
  }
  .re:hover {
    background: var(--term-line2);
  }
  .err {
    padding: 10px 16px;
    color: var(--term-9);
    font: 12px var(--mono);
  }
  .host {
    flex: 1;
    min-height: 0;
    padding: 10px 6px 6px 14px;
  }
  .host :global(.xterm-viewport) {
    background: transparent !important;
  }
</style>
