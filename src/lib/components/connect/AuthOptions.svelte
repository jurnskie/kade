<script lang="ts" module>
  export type AuthChoice = "one_password" | "agent" | "key_file" | "password" | "one_password_secret";
</script>

<script lang="ts">
  import { KeyRound, RectangleEllipsis } from "@lucide/svelte";
  import { t } from "$lib/i18n.svelte";
  import OnePasswordIcon from "../OnePasswordIcon.svelte";
  import AgentKeys from "./AgentKeys.svelte";
  import OnePasswordPicker from "./OnePasswordPicker.svelte";
  import type { OnePassword } from "./onepassword.svelte";
  import { radioGroup } from "./radioGroup";

  let {
    op,
    isFtp,
    choice = $bindable(),
    pinned = $bindable(),
    keyItem = $bindable(),
    keyPath = $bindable(),
    reference = $bindable(),
    user = $bindable(),
  }: {
    op: OnePassword;
    /** FTP has no SSH keys, so only the password options are offered. */
    isFtp: boolean;
    choice: AuthChoice;
    pinned: string | null;
    keyItem: string | null;
    keyPath: string;
    reference: string;
    user: string;
  } = $props();
</script>

<div class="auth" role="radiogroup" aria-label={t("Log in with")} use:radioGroup>
  {#if !isFtp}
    <div class="opt" class:on={choice === "one_password"}>
      <button
        class="head"
        role="radio"
        aria-checked={choice === "one_password"}
        tabindex={choice === "one_password" ? 0 : -1}
        onclick={() => ((choice = "one_password"), (pinned = null), (keyItem = null))}
      >
        <span class="r"></span>
        <span class="t">
          <b><OnePasswordIcon size={16} />1Password <span class="tag">{t("RECOMMENDED")}</span></b>
          <small>{t("The key stays in your vault — 1Password asks for approval when connecting.")}</small>
        </span>
      </button>
      {#if choice === "one_password"}
        <!-- Without the 1Password CLI, fall back to the keys its SSH agent offers. -->
        {#if op.accounts?.length === 0}
          <AgentKeys method="one_password" bind:pinned inset />
        {:else}
          <div class="sub"><OnePasswordPicker {op} listing="keys" bind:pinned bind:keyItem bind:reference bind:user /></div>
        {/if}
      {/if}
    </div>

    <div class="opt" class:on={choice === "agent" || choice === "key_file"}>
      <button
        class="head"
        role="radio"
        aria-checked={choice === "agent" || choice === "key_file"}
        tabindex={choice === "agent" || choice === "key_file" ? 0 : -1}
        onclick={() => ((choice = "agent"), (pinned = null))}
      >
        <span class="r"></span>
        <span class="t">
          <b><KeyRound size={16} color="var(--ink2)" />{t("SSH key")}</b>
          <small>{t("Via the ssh-agent (SSH_AUTH_SOCK) or a key file")}</small>
        </span>
      </button>
      {#if choice === "agent" || choice === "key_file"}
        <div class="sub">
          <div class="seg" role="group" aria-label={t("SSH key")}>
            <button class:on={choice === "agent"} aria-pressed={choice === "agent"} onclick={() => (choice = "agent")}>ssh-agent</button>
            <button class:on={choice === "key_file"} aria-pressed={choice === "key_file"} onclick={() => (choice = "key_file")}>
              {t("File")}
            </button>
          </div>
          {#if choice === "agent"}
            <AgentKeys method="agent" bind:pinned />
          {:else}
            <div class="in"><input class="mono" bind:value={keyPath} spellcheck="false" /></div>
            <p class="hint">{t("Encrypted key? Kade asks for the passphrase when connecting.")}</p>
          {/if}
        </div>
      {/if}
    </div>
  {/if}

  <div class="opt" class:on={choice === "one_password_secret"}>
    <button
      class="head"
      role="radio"
      aria-checked={choice === "one_password_secret"}
      tabindex={choice === "one_password_secret" ? 0 : -1}
      onclick={() => (choice = "one_password_secret")}
    >
      <span class="r"></span>
      <span class="t">
        <b>
          <OnePasswordIcon size={16} />{t("Password from 1Password")}
          {#if isFtp}<span class="tag">{t("RECOMMENDED")}</span>{/if}
        </b>
        <small>{t("Kade reads the password from your vault when connecting and only stores the reference.")}</small>
      </span>
    </button>
    {#if choice === "one_password_secret"}
      <div class="sub"><OnePasswordPicker {op} listing="logins" bind:pinned bind:keyItem bind:reference bind:user /></div>
    {/if}
  </div>

  <div class="opt" class:on={choice === "password"}>
    <button
      class="head"
      role="radio"
      aria-checked={choice === "password"}
      tabindex={choice === "password" ? 0 : -1}
      onclick={() => (choice = "password")}
    >
      <span class="r"></span>
      <span class="t">
        <b><RectangleEllipsis size={16} color="var(--ink2)" />{t("Password")}</b>
        <small>{t("Asked for every time you connect, never stored")}</small>
      </span>
    </button>
  </div>
</div>

<style>
  .auth {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .opt {
    background: var(--paper);
    border: 1px solid var(--mist);
    border-radius: 12px;
  }
  .opt.on {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
  }
  .head {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    padding: 12px 14px;
    text-align: left;
    width: 100%;
  }
  .r {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 1.5px solid var(--control);
    flex: none;
    margin-top: 2px;
  }
  .opt.on .r {
    border: 5px solid var(--pine);
  }
  .t b {
    font-weight: 600;
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .t small {
    display: block;
    color: var(--lichen);
    font-size: 12px;
    margin-top: 2px;
  }
  .tag {
    font: 500 10px var(--mono);
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--pine-t);
    color: var(--pine);
  }
  .sub {
    padding: 0 14px 12px 42px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .seg {
    display: flex;
    background: var(--mist2);
    border-radius: 8px;
    padding: 2px;
    align-self: flex-start;
  }
  .seg button {
    padding: 4px 10px;
    border-radius: 6px;
    font-weight: 500;
    color: var(--ink2);
  }
  .seg button.on {
    background: var(--paper);
    color: var(--granite);
    box-shadow: var(--shadow-sm);
  }
</style>
