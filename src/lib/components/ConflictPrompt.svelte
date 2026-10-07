<script lang="ts">
  import { TriangleAlert } from "@lucide/svelte";
  import type { ConflictAsk } from "$lib/tabs.svelte";
  import { t } from "$lib/i18n.svelte";
  import Prompt from "./Prompt.svelte";

  let { ask }: { ask: ConflictAsk } = $props();
</script>

<Prompt icon={TriangleAlert} color="var(--amber)" title={t("Already exists")} onclose={() => ask.resolve(null)}>
  {#snippet message()}
    {ask.names.length === 1
      ? t("{name} already exists in {dest}.", { name: ask.names[0], dest: ask.dest })
      : t("{n} items already exist in {dest}.", { n: ask.names.length, dest: ask.dest })}
    {t("Overwritten files go to Backups, so you can restore them.")}
  {/snippet}
  {#snippet actions()}
    <button class="btn ghost" onclick={() => ask.resolve(null)}>{t("Cancel")}</button>
    <button class="btn" onclick={() => ask.resolve("skip")}>{t("Skip")}</button>
    <button class="btn" onclick={() => ask.resolve("newer")}>{t("Only newer")}</button>
    <button class="btn pri" onclick={() => ask.resolve("overwrite")}>{t("Overwrite")}</button>
  {/snippet}
</Prompt>
