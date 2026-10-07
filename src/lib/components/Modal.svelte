<script lang="ts">
  import type { Snippet } from "svelte";
  import { X } from "@lucide/svelte";
  import { t } from "$lib/i18n.svelte";

  let {
    onclose,
    width,
    labelledby,
    z = 10,
    pad,
    gap = 0,
    top,
    blur = true,
    ring = false,
    escape = true,
    header,
    children,
  }: {
    /** Clicking the scrim, the close button and (unless `escape` is off) Escape. */
    onclose: () => void;
    /** Maximum width in px; on narrow windows the sheet keeps 16px from the edges. */
    width: number;
    labelledby?: string;
    /** z-index of the scrim; the sheet sits one above. Prompts that can pop up
     *  while another dialog is open (a connection started over MCP) go higher. */
    z?: number;
    /** Padding of a simple sheet whose children stack with `gap` px between them.
     *  Without it the children bring their own padding and scrolling. */
    pad?: string;
    gap?: number;
    /** Hang the sheet this far from the top instead of centring it. */
    top?: string;
    blur?: boolean;
    /** A hairline around the sheet, for large sheets that need a crisp edge. */
    ring?: boolean;
    escape?: boolean;
    /** The dialog's own header; it gets the close button to place in its layout. */
    header?: Snippet<[close: Snippet]>;
    children: Snippet;
  } = $props();
</script>

<svelte:window onkeydown={(e) => escape && e.key === "Escape" && onclose()} />

<div class="scrim" class:blur role="presentation" style:z-index={z} onclick={onclose}></div>
<div
  class="sheet"
  class:padded={pad != null}
  class:hung={top != null}
  class:ring
  role="dialog"
  aria-modal="true"
  aria-labelledby={labelledby}
  style:z-index={z + 1}
  style:width="min({width}px, calc(100vw - 32px))"
  style:top
  style:padding={pad}
  style:gap={pad != null ? `${gap}px` : undefined}
>
  {@render header?.(close)}
  {@render children()}
</div>

{#snippet close()}
  <button class="x" onclick={onclose} aria-label={t("Close")}><X size={16} /></button>
{/snippet}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
  }
  .scrim.blur {
    backdrop-filter: blur(3px);
  }
  .sheet {
    position: fixed;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    max-height: calc(100vh - 32px);
    display: flex;
    flex-direction: column;
    background: var(--paper);
    border-radius: 14px;
    box-shadow: var(--shadow-lg);
    overflow: hidden;
    user-select: text;
  }
  .sheet.padded {
    overflow-y: auto;
  }
  .sheet.hung {
    transform: translateX(-50%);
  }
  .sheet.ring {
    box-shadow: var(--shadow-lg), 0 0 0 1px var(--mist);
  }
  .x {
    margin-left: auto;
    padding: 6px;
    border-radius: 6px;
  }
  .x:hover {
    background: var(--mist2);
  }
</style>
