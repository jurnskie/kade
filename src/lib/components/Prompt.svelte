<script lang="ts">
  import type { Component, Snippet } from "svelte";
  import Modal from "./Modal.svelte";

  let {
    icon: Icon,
    color,
    title,
    onclose,
    message,
    children,
    actions,
  }: {
    icon: Component<{ size?: number; color?: string }>;
    color: string;
    title: string;
    onclose: () => void;
    message: Snippet;
    /** Anything between the message and the buttons, e.g. an input. */
    children?: Snippet;
    actions: Snippet;
  } = $props();
</script>

<!-- Prompts can pop up while another dialog is open, so they sit above it. -->
<Modal width={480} z={20} pad="22px 24px 18px" gap={12} escape={false} {onclose}>
  <div class="ph"><Icon size={20} {color} /><h3>{title}</h3></div>
  <p>{@render message()}</p>
  {@render children?.()}
  <div class="pa">{@render actions()}</div>
</Modal>

<style>
  .ph {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  h3 {
    font-size: 16px;
    font-weight: 600;
  }
  p {
    color: var(--ink2);
    line-height: 1.5;
  }
  .pa {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
</style>
