<script lang="ts" module>
  export interface PickRow {
    id: string;
    title: string;
    detail: string;
    /** Show the detail in the monospace font (fingerprints). */
    mono?: boolean;
    on: boolean;
    pick: () => void;
  }
</script>

<script lang="ts">
  import { LoaderCircle } from "@lucide/svelte";
  import { t } from "$lib/i18n.svelte";

  let {
    rows,
    loading,
    error,
    empty,
    onretry,
    inset = false,
  }: {
    rows: PickRow[];
    /** Shown with a spinner while the list loads. */
    loading: string | null;
    error: string | null;
    empty: string;
    /** Shows a Retry button next to the error. */
    onretry?: () => void;
    /** Indent under an option's heading, when not inside its sub-section. */
    inset?: boolean;
  } = $props();
</script>

<div class="keys" class:inset>
  {#if loading}
    <div class="keymsg"><LoaderCircle size={14} class="spin" />{loading}</div>
  {:else if error}
    <div class="keymsg err">
      <span>{error}</span>
      {#if onretry}<button class="retry" onclick={onretry}>{t("Retry")}</button>{/if}
    </div>
  {:else if rows.length === 0}
    <div class="keymsg">{empty}</div>
  {:else}
    {#each rows as row (row.id)}
      <button class="key" class:on={row.on} onclick={row.pick}>
        <span class="kr"></span>
        <span>
          <b>{row.title}</b>
          <small class:mono={row.mono}>{row.detail}</small>
        </span>
      </button>
    {/each}
  {/if}
</div>

<style>
  .keys {
    border: 1px solid var(--mist);
    border-radius: 10px;
    background: var(--snow);
    max-height: 180px;
    /* Reserved, so the dialog doesn't jump when the list arrives. */
    min-height: 120px;
    overflow-y: auto;
  }
  .keys.inset {
    margin: 0 14px 12px 42px;
  }
  .key {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    width: 100%;
    padding: 8px 12px;
    text-align: left;
  }
  .key + .key {
    border-top: 1px solid var(--mist2);
  }
  .key:hover {
    background: var(--paper);
  }
  .key b {
    display: block;
    font-weight: 600;
    font-size: 12.5px;
  }
  .key small {
    display: block;
    font-size: 11px;
    color: var(--lichen);
    margin-top: 1px;
  }
  .kr {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 1.5px solid var(--control);
    flex: none;
    margin-top: 3px;
  }
  .key.on .kr {
    border: 4px solid var(--pine);
  }
  .keymsg {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    color: var(--lichen);
    font-size: 12px;
  }
  .keymsg.err {
    color: var(--danger);
    align-items: flex-start;
  }
  .retry {
    margin-left: auto;
    flex: none;
    padding: 2px 8px;
    border: 1px solid var(--mist);
    border-radius: 6px;
    background: var(--paper);
    color: var(--granite);
    font-size: 12px;
  }
</style>
