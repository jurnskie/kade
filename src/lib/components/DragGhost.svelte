<script lang="ts">
  import { drag } from "$lib/drag.svelte";
  import { t, tn } from "$lib/i18n.svelte";
</script>

{#if drag.source}
  <div class="drag-ghost" style:left="{drag.x + 14}px" style:top="{drag.y + 10}px">
    {drag.source.paths.length === 1 ? drag.source.paths[0].split("/").pop() : tn(drag.source.paths.length, "{n} item", "{n} items")}
    {#if drag.over && drag.over.side !== drag.source.side}
      <span>→ {drag.over.side === "remote" ? t("upload") : t("download")}</span>
    {/if}
  </div>
{/if}

<style>
  .drag-ghost {
    position: fixed;
    z-index: 50;
    pointer-events: none;
    background: var(--inverse);
    color: var(--on-inverse);
    padding: 6px 10px;
    border-radius: 8px;
    font-weight: 500;
    font-size: 12.5px;
    box-shadow: var(--shadow-md);
    white-space: nowrap;
  }
  .drag-ghost span {
    opacity: 0.7;
    margin-left: 6px;
    font-family: var(--mono);
    font-size: 11px;
  }
</style>
