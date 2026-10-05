<script lang="ts">
  import {
    PlugZap,
    X,
    FolderLock,
    SquareTerminal,
    ShieldCheck,
    FolderUp,
    Globe,
    Laptop,
    KeyRound,
    RectangleEllipsis,
    LoaderCircle,
    Trash2,
  } from "@lucide/svelte";
  import { api, errorMessage, type AgentKey, type Auth, type OpAccount, type OpItem, type OpSshKey, type OpVault, type Protocol, type ServerProfile, type Workspace } from "$lib/api";
  import { colorOf, workspaceIdOf } from "$lib/workspaces";
  import { t } from "$lib/i18n.svelte";
  import OnePasswordIcon from "./OnePasswordIcon.svelte";

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

  type AuthChoice = "one_password" | "agent" | "key_file" | "password" | "one_password_secret";

  // The dialog is keyed by `initial` in the parent, so reading it once is intended.
  // svelte-ignore state_referenced_locally
  const start = initial;
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
  const wsAccount = $derived(workspaces.find((w) => w.id === workspace)?.op_account ?? "");
  // Follow the workspace's 1Password account until the user picks one explicitly.
  let accountPicked = false;
  $effect(() => {
    const acc = wsAccount;
    if (!accountPicked && acc && opAccounts?.some((a) => a.id === acc)) opAccount = acc;
  });
  let authChoice = $state<AuthChoice>(start?.auth.method ?? "one_password");
  let keyPath = $state(start?.auth.method === "key_file" ? start.auth.path : "~/.ssh/id_ed25519");
  let reference = $state(start?.auth.method === "one_password_secret" ? start.auth.reference : "");
  let opItems = $state<OpItem[] | null>(null);
  let opLoading = $state(false);
  let opError = $state<string | null>(null);
  let opQuery = $state("");

  // Which 1Password account and vault to use. A machine can be signed in to
  // several accounts (work and personal); without one, `op` picks its default.
  const startAuth = start?.auth;
  let opAccounts = $state<OpAccount[] | null>(null);
  let opAccount = $state<string>(
    (startAuth?.method === "one_password" || startAuth?.method === "one_password_secret") && startAuth.account
      ? startAuth.account
      : "",
  );
  let opVaults = $state<OpVault[]>([]);
  let opVault = $state<string>("");
  let opKeys = $state<OpSshKey[] | null>(null);
  let keyItem = $state<string | null>(startAuth?.method === "one_password" ? (startAuth.key_item ?? null) : null);
  const usesOp = $derived(authChoice === "one_password" || authChoice === "one_password_secret");
  let pinned = $state<string | null>(
    start && (start.auth.method === "one_password" || start.auth.method === "agent") ? start.auth.key_fingerprint : null,
  );

  let keys = $state<AgentKey[]>([]);
  let keysLoading = $state(false);
  let keysError = $state<string | null>(null);

  const protocols: { id: Protocol; label: string; hint: string; icon: typeof FolderLock; ready: boolean }[] = [
    { id: "sftp", label: "SFTP", hint: "Files", icon: FolderLock, ready: true },
    { id: "ssh", label: "SSH", hint: "Terminal only", icon: SquareTerminal, ready: true },
    { id: "ftps", label: "FTPS", hint: "FTP with TLS", icon: ShieldCheck, ready: true },
    { id: "ftp", label: "FTP", hint: "Unencrypted", icon: FolderUp, ready: true },
  ];

  const isFtp = $derived(protocol === "ftp" || protocol === "ftps");

  /** Switch protocol, moving the port along when it is still a default. */
  function setProtocol(p: Protocol) {
    const ftp = p === "ftp" || p === "ftps";
    if ([21, 22, 990].includes(port)) port = ftp ? 21 : 22;
    // FTP has no SSH keys: fall back to a password from 1Password.
    if (ftp && ["one_password", "agent", "key_file"].includes(authChoice)) authChoice = "one_password_secret";
    protocol = p;
  }

  // Accounts once, when a 1Password option is first picked.
  $effect(() => {
    if (!usesOp || opAccounts) return;
    api
      .opAccounts()
      .then((list) => {
        opAccounts = list;
        if (!list.some((a) => a.id === opAccount)) {
          opAccount = list.some((a) => a.id === wsAccount) ? wsAccount : (list[0]?.id ?? "");
        }
      })
      .catch((e) => {
        opAccounts = [];
        opError = errorMessage(e);
      });
  });

  // Vaults of the chosen account.
  $effect(() => {
    const account = opAccount;
    if (!usesOp || !account) return;
    opVaults = [];
    api.opVaults(account).then((v) => {
      opVaults = v;
      if (opVault && !v.some((x) => x.id === opVault)) opVault = "";
    });
  });

  // Logins (for passwords) or SSH keys in the chosen account/vault.
  $effect(() => {
    const [account, vault, choice] = [opAccount, opVault, authChoice];
    if (!account) return;
    if (choice !== "one_password_secret" && choice !== "one_password") return;
    opLoading = true;
    opError = null;
    const done = () => (opLoading = false);
    if (choice === "one_password_secret") {
      opItems = null;
      api.opItems(account, vault || null).then((i) => (opItems = i)).catch((e) => (opError = errorMessage(e))).finally(done);
    } else {
      opKeys = null;
      api.opSshKeys(account, vault || null).then((k) => (opKeys = k)).catch((e) => (opError = errorMessage(e))).finally(done);
    }
  });

  function pickOpKey(k: OpSshKey) {
    pinned = k.fingerprint;
    keyItem = k.item;
  }

  const opMatches = $derived.by(() => {
    const q = opQuery.trim().toLowerCase();
    const items = opItems ?? [];
    const hits = q
      ? items.filter((i) => `${i.title} ${i.username ?? ""} ${i.url ?? ""} ${i.vault}`.toLowerCase().includes(q))
      : items;
    return hits.slice(0, 40);
  });
  const opSelected = $derived(opItems?.find((i) => i.reference === reference) ?? null);

  function pickOpItem(item: OpItem) {
    reference = item.reference;
    if (!user.trim() && item.username) user = item.username;
  }

  // The agent's keys: for "agent", or as fallback when the 1Password CLI is missing.
  $effect(() => {
    if (authChoice !== "agent" && !(authChoice === "one_password" && opAccounts?.length === 0)) return;
    const auth: Auth =
      authChoice === "agent" ? { method: "agent", key_fingerprint: null } : { method: "one_password", key_fingerprint: null };
    keysLoading = true;
    keysError = null;
    keys = [];
    api
      .agentKeys(auth)
      .then((k) => (keys = k))
      .catch((e) => (keysError = errorMessage(e)))
      .finally(() => (keysLoading = false));
  });

  /** Accept pasted `sftp://user@host:port/path` or `user@host`. */
  function onHostPaste(e: ClipboardEvent) {
    const text = e.clipboardData?.getData("text")?.trim() ?? "";
    const m = text.match(/^(?:(sftp|ssh|ftps?):\/\/)?(?:([^@\s/]+)@)?([^:\s/]+)(?::(\d+))?(\/\S*)?$/);
    if (!m || (!m[1] && !m[2])) return;
    e.preventDefault();
    if (m[1]) protocol = m[1] as Protocol;
    if (m[2]) user = m[2];
    host = m[3];
    if (m[4]) port = Number(m[4]);
    if (m[5]) remotePath = m[5];
    if (!name) name = m[3];
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
        auth = { method: "one_password", key_fingerprint: pinned, account: opAccount || null, key_item: keyItem };
        break;
      case "agent":
        auth = { method: "agent", key_fingerprint: pinned };
        break;
      case "key_file":
        auth = { method: "key_file", path: keyPath.trim() };
        break;
      case "one_password_secret":
        auth = { method: "one_password_secret", reference, account: opAccount || null };
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

  function submit(connect: boolean) {
    if (valid) onsave(build(), connect);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") oncancel();
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) submit(true);
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="scrim" role="presentation" onclick={oncancel}></div>
<div class="sheet" role="dialog" aria-modal="true" aria-labelledby="dlg-title">
  <div class="sh">
    <div class="ic"><PlugZap size={20} color="var(--pine)" /></div>
    <div>
      <h1 id="dlg-title">{initial ? t("Edit connection") : t("New connection")}</h1>
      <p>{t("Paste a URL like sftp://user@host:22/path into the host field — Kade fills in the rest.")}</p>
    </div>
    <button class="x" onclick={oncancel} aria-label={t("Close")}><X size={16} /></button>
  </div>

  <div class="body">
    <div class="col">
      <div class="lbl">Protocol</div>
      <div class="proto">
        {#each protocols as p (p.id)}
          <button class:on={protocol === p.id} disabled={!p.ready} onclick={() => setProtocol(p.id)}>
            <p.icon size={16} color={protocol === p.id ? "var(--pine)" : "var(--ink2)"} />
            <b>{p.label}</b><small>{t(p.hint)}</small>
          </button>
        {/each}
      </div>
      {#if protocol === "ftp"}
        <p class="warn">
          <b>{t("Unencrypted.")}</b>
          {t("Your password and files travel over the network in plain text. Choose FTPS if the server supports it, or SFTP if you have SSH access.")}
        </p>
      {/if}

      <label class="f">
        <span>{t("Name")}</span>
        <div class="in"><input bind:value={name} placeholder={host || "my-server"} /></div>
      </label>
      <div class="row">
        <label class="f">
          <span>Host</span>
          <div class="in">
            <Globe size={14} color="var(--lichen)" />
            <input class="mono" bind:value={host} onpaste={onHostPaste} placeholder="server.example.com" spellcheck="false" />
          </div>
        </label>
        <label class="f">
          <span>{t("Port")}</span>
          <div class="in"><input class="mono" type="number" bind:value={port} min="1" max="65535" /></div>
        </label>
      </div>
      <div class="row2">
        <label class="f">
          <span>{t("User")}</span>
          <div class="in"><input class="mono" bind:value={user} placeholder="deploy" spellcheck="false" /></div>
        </label>
        <label class="f">
          <span>{t("Group")}</span>
          <div class="in"><input bind:value={group} list="groups" placeholder="Homelab" /></div>
          <datalist id="groups">{#each groups as g (g)}<option value={g}></option>{/each}</datalist>
        </label>
      </div>
      {#if workspaces.length > 1}
        <div class="f">
          <span>Workspace</span>
          <div class="wspick">
            {#each workspaces as w (w.id)}
              <button
                class:on={workspace === w.id}
                style:--ws={colorOf(w).color}
                style:--ws-tint={colorOf(w).tint}
                onclick={() => (workspace = w.id)}
              >
                <span class="d"></span>{w.name}
              </button>
            {/each}
          </div>
        </div>
      {/if}
      <label class="f">
        <span>{t("Start folder on server")} <em>· {t("optional")}</em></span>
        <div class="in"><input class="mono" bind:value={remotePath} placeholder={t("home folder")} spellcheck="false" /></div>
      </label>
      <label class="f">
        <span>{t("Link a local folder")} <em>· {t("optional")}</em></span>
        <div class="in">
          <Laptop size={14} color="var(--lichen)" />
          <input class="mono" bind:value={localPath} placeholder="~/Sites" spellcheck="false" />
        </div>
      </label>
    </div>

    <div class="col alt">
      <div class="lbl">{t("Log in with")}</div>
      <div class="auth">
        {#if !isFtp}
        <div class="opt" class:on={authChoice === "one_password"}>
          <button class="head" onclick={() => ((authChoice = "one_password"), (pinned = null), (keyItem = null))}>
            <span class="r"></span>
            <span class="t">
              <b><OnePasswordIcon size={16} />1Password <span class="tag">{t("RECOMMENDED")}</span></b>
              <small>{t("The key stays in your vault — 1Password asks for approval when connecting.")}</small>
            </span>
          </button>
          {#if authChoice === "one_password"}
            {#if opAccounts?.length === 0}
              {@render keyPicker()}
            {:else}
              {@render opKeyPicker()}
            {/if}
          {/if}
        </div>

        <div class="opt" class:on={authChoice === "agent" || authChoice === "key_file"}>
          <button class="head" onclick={() => ((authChoice = "agent"), (pinned = null))}>
            <span class="r"></span>
            <span class="t">
              <b><KeyRound size={16} color="var(--ink2)" />{t("SSH key")}</b>
              <small>{t("Via the ssh-agent (SSH_AUTH_SOCK) or a key file")}</small>
            </span>
          </button>
          {#if authChoice === "agent" || authChoice === "key_file"}
            <div class="sub">
              <div class="seg">
                <button class:on={authChoice === "agent"} onclick={() => (authChoice = "agent")}>ssh-agent</button>
                <button class:on={authChoice === "key_file"} onclick={() => (authChoice = "key_file")}>{t("File")}</button>
              </div>
              {#if authChoice === "agent"}
                {@render keyPicker()}
              {:else}
                <div class="in"><input class="mono" bind:value={keyPath} spellcheck="false" /></div>
                <p class="hint">{t("Encrypted key? Kade asks for the passphrase when connecting.")}</p>
              {/if}
            </div>
          {/if}
        </div>

        {/if}

        <div class="opt" class:on={authChoice === "one_password_secret"}>
          <button class="head" onclick={() => (authChoice = "one_password_secret")}>
            <span class="r"></span>
            <span class="t">
              <b>
                <OnePasswordIcon size={16} />{t("Password from 1Password")}
                {#if isFtp}<span class="tag">{t("RECOMMENDED")}</span>{/if}
              </b>
              <small>{t("Kade reads the password from your vault when connecting and only stores the reference.")}</small>
            </span>
          </button>
          {#if authChoice === "one_password_secret"}
            {@render opPicker()}
          {/if}
        </div>

        <div class="opt" class:on={authChoice === "password"}>
          <button class="head" onclick={() => (authChoice = "password")}>
            <span class="r"></span>
            <span class="t">
              <b><RectangleEllipsis size={16} color="var(--ink2)" />{t("Password")}</b>
              <small>{t("Asked for every time you connect, never stored")}</small>
            </span>
          </button>
        </div>
      </div>
    </div>
  </div>

  <div class="ft">
    {#if initial}
      <button class="btn ghost danger" onclick={() => ondelete(initial)}><Trash2 size={15} />{t("Delete")}</button>
    {/if}
    <div class="actions">
      <button class="btn ghost" onclick={oncancel}>{t("Cancel")}</button>
      <button class="btn" disabled={!valid} onclick={() => submit(false)}>{t("Save")}</button>
      <button class="btn pri" disabled={!valid} onclick={() => submit(true)}>
        {t("Save & connect")} <kbd>Ctrl+Enter</kbd>
      </button>
    </div>
  </div>
</div>

{#snippet opScope()}
  <div class="scope">
    {#if (opAccounts?.length ?? 0) > 1}
      <select bind:value={opAccount} title={t("1Password account")} onchange={() => (accountPicked = true)}>
        {#each opAccounts ?? [] as a (a.id)}<option value={a.id}>{a.url}{a.email ? ` · ${a.email}` : ""}</option>{/each}
      </select>
    {:else if opAccounts?.length === 1}
      <span class="acct" title={opAccounts[0].email}>{opAccounts[0].url}</span>
    {/if}
    <select bind:value={opVault} title={t("Vault")}>
      <option value="">{t("All vaults")}</option>
      {#each opVaults as v (v.id)}<option value={v.id}>{v.name}</option>{/each}
    </select>
  </div>
{/snippet}

{#snippet opKeyPicker()}
  <div class="sub">
    {@render opScope()}
    <div class="keys">
      {#if opLoading}
        <div class="keymsg"><LoaderCircle size={14} class="spin" />{t("Fetching keys…")}</div>
      {:else if opError}
        <div class="keymsg err">{opError}</div>
      {:else if (opKeys ?? []).length === 0}
        <div class="keymsg">{t("No SSH keys in this vault.")}</div>
      {:else}
        {#each opKeys ?? [] as k (k.item)}
          <button class="key" class:on={keyItem === k.item || (!keyItem && pinned === k.fingerprint)} onclick={() => pickOpKey(k)}>
            <span class="kr"></span>
            <span>
              <b>{k.title}</b>
              <small class="mono">{k.vault} · {k.fingerprint.replace("SHA256:", "").slice(0, 12)}…</small>
            </span>
          </button>
        {/each}
      {/if}
    </div>
    <p class="hint">
      {t("Kade tries the 1Password SSH agent first. If it doesn't offer the key on this computer (e.g. a work laptop), Kade fetches it from this account and vault.")}
    </p>
  </div>
{/snippet}

{#snippet opPicker()}
  <div class="sub">
    {@render opScope()}
    {#if opSelected}
      <div class="picked">
        <OnePasswordIcon size={14} />
        <span><b>{opSelected.title}</b><small>{opSelected.vault}{opSelected.username ? ` · ${opSelected.username}` : ""}</small></span>
      </div>
    {:else if reference}
      <div class="picked"><OnePasswordIcon size={14} /><span class="mono small">{reference}</span></div>
    {/if}
    <div class="in"><input bind:value={opQuery} placeholder={t("Search logins in 1Password…")} spellcheck="false" /></div>
    <div class="keys">
      {#if opLoading}
        <div class="keymsg"><LoaderCircle size={14} class="spin" />{t("Fetching logins… (1Password may ask for approval)")}</div>
      {:else if opError}
        <div class="keymsg err">{opError}</div>
      {:else if opMatches.length === 0}
        <div class="keymsg">{t("No logins found.")}</div>
      {:else}
        {#each opMatches as item (item.reference)}
          <button class="key" class:on={reference === item.reference} onclick={() => pickOpItem(item)}>
            <span class="kr"></span>
            <span>
              <b>{item.title}</b>
              <small>{item.vault}{item.username ? ` · ${item.username}` : ""}{item.url ? ` · ${item.url}` : ""}</small>
            </span>
          </button>
        {/each}
      {/if}
    </div>
  </div>
{/snippet}

{#snippet keyPicker()}
  <div class="keys">
    {#if keysLoading}
      <div class="keymsg"><LoaderCircle size={14} class="spin" />{t("Fetching keys…")}</div>
    {:else if keysError}
      <div class="keymsg err">{keysError}</div>
    {:else if keys.length === 0}
      <div class="keymsg">{t("No keys found in the agent.")}</div>
    {:else}
      <button class="key" class:on={pinned === null} onclick={() => (pinned = null)}>
        <span class="kr"></span>
        <span><b>{t("Try every key")}</b><small>{t("May hit the server's MaxAuthTries limit")}</small></span>
      </button>
      {#each keys as k (k.fingerprint)}
        <button class="key" class:on={pinned === k.fingerprint} onclick={() => (pinned = k.fingerprint)}>
          <span class="kr"></span>
          <span>
            <b>{k.comment || t("Unnamed key")}</b>
            <small class="mono">{k.algorithm} · {k.fingerprint.replace("SHA256:", "").slice(0, 12)}…</small>
          </span>
        </button>
      {/each}
    {/if}
  </div>
{/snippet}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    backdrop-filter: blur(3px);
    z-index: 10;
  }
  .sheet {
    position: fixed;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    width: min(960px, calc(100vw - 40px));
    max-height: calc(100vh - 40px);
    display: flex;
    flex-direction: column;
    background: var(--paper);
    border-radius: 16px;
    box-shadow: var(--shadow-lg), 0 0 0 1px var(--mist);
    overflow: hidden;
    z-index: 11;
    user-select: text;
  }
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
  .x {
    margin-left: auto;
    padding: 6px;
    border-radius: 6px;
    color: var(--ink2);
  }
  .x:hover {
    background: var(--mist2);
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
  .proto {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 6px;
    margin-bottom: 20px;
  }
  .proto button {
    border: 1px solid var(--mist);
    border-radius: 10px;
    padding: 10px 10px 9px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    text-align: left;
  }
  .proto button b {
    font-weight: 600;
  }
  .proto button small {
    font-size: 11px;
    color: var(--lichen);
  }
  .proto button.on {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
  }
  .proto button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .f {
    display: block;
    margin-bottom: 14px;
  }
  .f > span {
    display: block;
    font-weight: 500;
    font-size: 12.5px;
    margin-bottom: 6px;
    color: var(--ink2);
  }
  .f em {
    font-style: normal;
    font-weight: 400;
    color: var(--lichen);
  }
  .in {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    border: 1px solid var(--mist);
    border-radius: 8px;
    padding: 0 11px;
    background: var(--paper);
  }
  .in:focus-within {
    border-color: var(--pine);
    box-shadow: 0 0 0 3px var(--pine-t);
  }
  .in input {
    border: 0;
    outline: 0;
    background: transparent;
    flex: 1;
    min-width: 0;
  }
  .in input.mono {
    font-size: 12.5px;
  }
  .in input::placeholder {
    color: var(--faint);
  }
  .row {
    display: grid;
    grid-template-columns: 1fr 96px;
    gap: 10px;
  }
  .row2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
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
  .wspick {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .wspick button {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 32px;
    padding: 0 12px;
    border-radius: 8px;
    border: 1px solid var(--mist);
    font-weight: 500;
  }
  .wspick button .d {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ws);
  }
  .wspick button.on {
    background: var(--ws-tint);
    border-color: var(--ws);
  }
  .scope {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
  }
  .scope select {
    height: 30px;
    border: 1px solid var(--mist);
    border-radius: 7px;
    padding: 0 8px;
    background: var(--paper);
    font-size: 12px;
    max-width: 100%;
  }
  .acct {
    font-size: 12px;
    color: var(--ink2);
    padding: 0 4px;
  }
  .warn {
    margin: -10px 0 18px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--amber-t);
    color: var(--amber-ink);
    font-size: 12px;
    line-height: 1.45;
  }
  .picked {
    display: flex;
    gap: 9px;
    align-items: center;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--pine-t);
  }
  .picked b {
    display: block;
    font-weight: 600;
    font-size: 12.5px;
  }
  .picked small {
    display: block;
    font-size: 11px;
    color: var(--ink2);
  }
  .small {
    font-size: 11px;
    word-break: break-all;
  }
  .hint {
    font-size: 11.5px;
    color: var(--lichen);
  }
  .keys {
    margin: 0 14px 12px 42px;
    border: 1px solid var(--mist);
    border-radius: 10px;
    background: var(--snow);
    max-height: 180px;
    overflow-y: auto;
  }
  .sub .keys {
    margin: 0;
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
    .proto {
      grid-template-columns: repeat(2, 1fr);
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
