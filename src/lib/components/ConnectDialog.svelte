<script lang="ts">
  import { PlugZap, Trash2 } from "@lucide/svelte";
  import type { Auth, Protocol, ServerProfile, Workspace } from "$lib/api";
  import { workspaceIdOf } from "$lib/workspaces";
  import { t } from "$lib/i18n.svelte";
  import { isMac } from "$lib/keys";
  import Modal from "./Modal.svelte";
  import ServerFields from "./connect/ServerFields.svelte";
  import AuthOptions, { type AuthChoice } from "./connect/AuthOptions.svelte";
  import { OnePassword } from "./connect/onepassword.svelte";

  let {
    initial,
    groups,
    workspaces,
    defaultWorkspace,
    onsave,
    oncancel,
    ondelete,
  }: {
    initial: ServerProfile | null;
    groups: string[];
    workspaces: Workspace[];
    defaultWorkspace: Workspace | null;
    onsave: (p: ServerProfile, connect: boolean) => void;
    oncancel: () => void;
    ondelete: (p: ServerProfile) => void;
  } = $props();

  // The dialog is keyed by `initial` in the parent, so reading it once is intended.
  // svelte-ignore state_referenced_locally
  const start = initial;
  const startAuth = start?.auth;
  let name = $state(start?.name ?? "");
  let protocol = $state<Protocol>(start?.protocol ?? "sftp");
  let host = $state(start?.host ?? "");
  let port = $state(start?.port ?? 22);
  let user = $state(start?.user ?? "");
  let group = $state(start?.group ?? "");
  let remotePath = $state(start?.remote_path ?? "");
  let localPath = $state(start?.local_path ?? "");
  // svelte-ignore state_referenced_locally
  let workspace = $state(start ? workspaceIdOf(start) : (defaultWorkspace?.id ?? ""));

  let authChoice = $state<AuthChoice>(startAuth?.method ?? "one_password");
  let keyPath = $state(startAuth?.method === "key_file" ? startAuth.path : "~/.ssh/id_ed25519");
  let reference = $state(startAuth?.method === "one_password_secret" ? startAuth.reference : "");
  let keyItem = $state<string | null>(startAuth?.method === "one_password" ? (startAuth.key_item ?? null) : null);
  let pinned = $state<string | null>(
    startAuth?.method === "one_password" || startAuth?.method === "agent" ? startAuth.key_fingerprint : null,
  );

  const ws = $derived(workspaces.find((w) => w.id === workspace));
  const wsAccount = $derived(ws?.op_account ?? "");
  const op = new OnePassword(
    (startAuth?.method === "one_password" || startAuth?.method === "one_password_secret") && startAuth.account
      ? startAuth.account
      : "",
    () => (authChoice === "one_password" ? "keys" : authChoice === "one_password_secret" ? "logins" : null),
    () => wsAccount,
    () => ws?.op_vault ?? "",
    () =>
      ws?.op_key_fingerprint
        ? { fingerprint: ws.op_key_fingerprint, item: ws.op_key_item, title: ws.op_key_title ?? null, account: ws.op_account ?? "" }
        : null,
    () => !!pinned || !!keyItem,
  );

  const isFtp = $derived(protocol === "ftp" || protocol === "ftps");

  /** Switch protocol, moving the port along when it is still a default. */
  function setProtocol(p: Protocol) {
    const ftp = p === "ftp" || p === "ftps";
    if ([21, 22, 990].includes(port)) port = ftp ? 21 : 22;
    // FTP has no SSH keys: fall back to a password from 1Password.
    if (ftp && ["one_password", "agent", "key_file"].includes(authChoice)) authChoice = "one_password_secret";
    protocol = p;
  }

  const valid = $derived(
    host.trim() !== "" &&
      user.trim() !== "" &&
      port > 0 &&
      port < 65536 &&
      (authChoice !== "one_password_secret" || reference.startsWith("op://")),
  );

  function build(): ServerProfile {
    let auth: Auth;
    switch (authChoice) {
      case "one_password":
        auth = { method: "one_password", key_fingerprint: pinned, account: op.account || null, key_item: keyItem };
        break;
      case "agent":
        auth = { method: "agent", key_fingerprint: pinned };
        break;
      case "key_file":
        auth = { method: "key_file", path: keyPath.trim() };
        break;
      case "one_password_secret":
        auth = { method: "one_password_secret", reference, account: op.account || null };
        break;
      default:
        auth = { method: "password" };
    }
    return {
      id: initial?.id ?? "",
      name: name.trim() || host.trim(),
      protocol,
      host: host.trim(),
      port,
      user: user.trim(),
      group: group.trim(),
      auth,
      remote_path: remotePath.trim() || null,
      local_path: localPath.trim() || null,
      workspace,
      tunnels: initial?.tunnels ?? [],
      updated_at: initial?.updated_at ?? 0,
    };
  }

  let saving = $state(false);

  /** `onsave` may be async; once called, further clicks must not create a second profile. */
  async function submit(connect: boolean) {
    if (!valid || saving) return;
    saving = true;
    try {
      await onsave(build(), connect);
    } finally {
      saving = false;
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Enter" && (e.ctrlKey || e.metaKey) && submit(true)} />

<Modal width={960} ring labelledby="dlg-title" onclose={oncancel}>
  {#snippet header(close)}
    <div class="sh">
      <div class="ic"><PlugZap size={20} color="var(--pine)" /></div>
      <div>
        <h1 id="dlg-title">{initial ? t("Edit connection") : t("New connection")}</h1>
        <p>{t("Paste a URL like sftp://user@host:22/path into the host field — Kade fills in the rest.")}</p>
      </div>
      {@render close()}
    </div>
  {/snippet}

  <div class="body">
    <div class="col">
      <div class="lbl">{t("Protocol")}</div>
      <ServerFields
        {protocol}
        onprotocol={setProtocol}
        {groups}
        {workspaces}
        bind:name
        bind:host
        bind:port
        bind:user
        bind:group
        bind:workspace
        bind:remotePath
        bind:localPath
      />
    </div>

    <div class="col alt">
      <div class="lbl">{t("Log in with")}</div>
      <AuthOptions {op} {isFtp} bind:choice={authChoice} bind:pinned bind:keyItem bind:keyPath bind:reference bind:user />
    </div>
  </div>

  <div class="ft">
    {#if initial}
      <button class="btn ghost danger" onclick={() => ondelete(initial)}><Trash2 size={15} />{t("Delete")}</button>
    {/if}
    <div class="actions">
      <button class="btn ghost" onclick={oncancel}>{t("Cancel")}</button>
      <button class="btn" disabled={!valid || saving} onclick={() => submit(false)}>{t("Save")}</button>
      <button class="btn pri" disabled={!valid || saving} onclick={() => submit(true)}>
        {t("Save & connect")} <kbd>{isMac ? "⌘ Enter" : "Ctrl+Enter"}</kbd>
      </button>
    </div>
  </div>
</Modal>

<style>
  .sh {
    display: flex;
    align-items: flex-start;
    gap: 14px;
    padding: 22px 26px 18px;
    border-bottom: 1px solid var(--mist2);
  }
  .ic {
    width: 40px;
    height: 40px;
    border-radius: 10px;
    background: var(--pine-t);
    display: grid;
    place-items: center;
    flex: none;
  }
  h1 {
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.015em;
  }
  .sh p {
    color: var(--lichen);
    margin-top: 3px;
  }
  .body {
    display: grid;
    grid-template-columns: 1fr 1fr;
    overflow-y: auto;
    min-height: 0;
  }
  .col {
    padding: 22px 26px;
  }
  .col.alt {
    background: var(--snow);
    border-left: 1px solid var(--mist2);
  }
  .lbl {
    margin-bottom: 8px;
  }
  /* Text fields and hints, shared by both columns. */
  .body :global(.in) {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 0 11px;
    background: var(--paper);
  }
  .body :global(.in:focus-within) {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
  }
  .body :global(.in input) {
    border: 0;
    outline: 0;
    background: transparent;
    flex: 1;
    min-width: 0;
  }
  .body :global(.in input.mono) {
    font-size: 12.5px;
  }
  .body :global(.in input::placeholder) {
    color: var(--lichen);
  }
  .body :global(.hint) {
    font-size: 11.5px;
    color: var(--lichen);
  }
  .ft {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 26px;
    border-top: 1px solid var(--mist2);
  }
  .actions {
    margin-left: auto;
    display: flex;
    gap: 8px;
  }
  @media (max-width: 820px) {
    .body {
      grid-template-columns: 1fr;
    }
    .col.alt {
      border-left: 0;
      border-top: 1px solid var(--mist2);
    }
    .sh,
    .col,
    .ft {
      padding-left: 18px;
      padding-right: 18px;
    }
    .ft kbd {
      display: none;
    }
  }
  kbd {
    font: 10.5px var(--mono);
    opacity: 0.7;
    margin-left: 2px;
  }
</style>
