<script lang="ts">
  import { Fingerprint } from "@lucide/svelte";
  import type { ServerProfile } from "$lib/api";
  import type { ConnectPrompt } from "$lib/tabs.svelte";
  import { t } from "$lib/i18n.svelte";
  import Prompt from "./Prompt.svelte";

  let {
    prompt,
    onclose,
    onconnect,
  }: {
    prompt: ConnectPrompt;
    onclose: () => void;
    onconnect: (server: ServerProfile, password?: string, acceptFingerprint?: string) => void;
  } = $props();

  let password = $state("");

  function trustHost() {
    if (prompt.kind !== "hostkey") return;
    const { server, fingerprint, password } = prompt;
    onclose();
    onconnect(server, password, fingerprint);
  }

  function submitPassword() {
    if (prompt.kind !== "password" || !password) return;
    const { server, acceptFingerprint } = prompt;
    const pw = password;
    password = "";
    onclose();
    onconnect(server, pw, acceptFingerprint);
  }
</script>

{#if prompt.kind === "hostkey"}
  <Prompt icon={Fingerprint} color="var(--amber)" title={t("Unknown server")} {onclose}>
    {#snippet message()}
      {t("Kade hasn't seen {host} before. Check the fingerprint before you continue — on the server, run:", { host: prompt.server.host })}
      <span class="mono cmd">ssh-keygen -lf /etc/ssh/ssh_host_*_key.pub</span>
    {/snippet}
    <div class="fp mono"><small>{prompt.algorithm}</small>{prompt.fingerprint}</div>
    {#snippet actions()}
      <button class="btn ghost" onclick={onclose}>{t("Cancel")}</button>
      <button class="btn pri" onclick={trustHost}>{t("Trust & connect")}</button>
    {/snippet}
  </Prompt>
{:else}
  <Prompt
    icon={Fingerprint}
    color="var(--pine)"
    title={prompt.server.auth.method === "key_file" ? t("Key passphrase") : t("Password")}
    {onclose}
  >
    {#snippet message()}
      {t("For {who}. Not stored.", { who: `${prompt.server.user}@${prompt.server.host}` })}
    {/snippet}
    <!-- svelte-ignore a11y_autofocus -->
    <input class="pw" type="password" bind:value={password} autofocus onkeydown={(e) => e.key === "Enter" && submitPassword()} />
    {#snippet actions()}
      <button class="btn ghost" onclick={onclose}>{t("Cancel")}</button>
      <button class="btn pri" disabled={!password} onclick={submitPassword}>{t("Connect")}</button>
    {/snippet}
  </Prompt>
{/if}

<style>
  .cmd {
    font-size: 11.5px;
    background: var(--mist2);
    padding: 1px 4px;
    border-radius: 4px;
  }
  .fp {
    background: var(--snow);
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 10px 12px;
    font-size: 12px;
    word-break: break-all;
  }
  .fp small {
    display: block;
    color: var(--lichen);
    font-size: 11px;
    margin-bottom: 2px;
  }
  .pw {
    height: 36px;
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 0 11px;
    outline: 0;
  }
  .pw:focus {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
  }
</style>
