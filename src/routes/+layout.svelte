<script lang="ts">
  import "@fontsource/instrument-sans/400.css";
  import "@fontsource/instrument-sans/500.css";
  import "@fontsource/instrument-sans/600.css";
  import "@fontsource/instrument-sans/700.css";
  import "@fontsource/jetbrains-mono/400.css";
  import "@fontsource/jetbrains-mono/500.css";
  import "$lib/styles.css";

  let { children } = $props();

  function onContextMenu(e: MouseEvent) {
    const el = e.target as HTMLElement;
    if (el.closest("input, textarea, [contenteditable]") && !el.closest(".xterm")) return;
    e.preventDefault();
  }
</script>

<svelte:window oncontextmenu={onContextMenu} />

{@render children()}
