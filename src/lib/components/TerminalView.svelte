<script lang="ts">
  import { onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";
  import { RotateCcw } from "@lucide/svelte";
  import { api, errorMessage } from "$lib/api";
  import { t } from "$lib/i18n.svelte";

  let { sessionId, visible, title }: { sessionId: string; visible: boolean; title: string } = $props();

  let host: HTMLDivElement;
  let term: Terminal;
  let fit: FitAddon;
  let termId: string | null = null;
  let exited = $state(false);
  let error = $state<string | null>(null);

  async function start() {
    exited = false;
    error = null;
    term.reset();
    fit.fit();
    try {
      termId = await api.terminalOpen(sessionId, term.cols, term.rows, (e) => {
        if (e.type === "data") term.write(new Uint8Array(e.bytes));
        else {
          exited = true;
          termId = null;
          term.write(`\r\n\x1b[2m[${t("session ended")}]\x1b[0m\r\n`);
        }
      });
      if (exited) termId = null;
      if (visible) term.focus();
    } catch (e) {
      error = errorMessage(e);
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
      theme: {
        background: "#0f1513",
        foreground: "#d9dfdb",
        cursor: "#5fc79e",
        cursorAccent: "#0f1513",
        selectionBackground: "#2a3d35",
        black: "#18221e",
        red: "#e06c5a",
        green: "#5fc79e",
        yellow: "#e0a84a",
        blue: "#8fb8e8",
        magenta: "#c79bd8",
        cyan: "#6fc3c0",
        white: "#d9dfdb",
        brightBlack: "#6b7872",
        brightRed: "#f08a78",
        brightGreen: "#7fdcb5",
        brightYellow: "#f0c070",
        brightBlue: "#aacdf2",
        brightMagenta: "#dab3e8",
        brightCyan: "#8fd8d5",
        brightWhite: "#f4f6f4",
      },
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);

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
      ro.disconnect();
      if (termId) api.terminalClose(termId).catch(() => {});
      term.dispose();
    };
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
    background: #0f1513;
    border-radius: 12px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 0 0 1px #1a2420;
    user-select: text;
  }
  .th {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-bottom: 1px solid #1c2723;
  }
  .st {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 5px 10px;
    border-radius: 6px;
    background: #18221e;
    color: #e8ece8;
    font: 500 12px var(--mono);
  }
  .st i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #5fc79e;
  }
  .st i.off {
    background: #6b7872;
  }
  .re {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    border-radius: 6px;
    background: #1a2420;
    border: 1px solid #233029;
    color: #c9d1cc;
    font: 500 12px var(--sans);
  }
  .re:hover {
    background: #233029;
  }
  .err {
    padding: 10px 16px;
    color: #f08a78;
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
