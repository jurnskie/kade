<script lang="ts">
  import { FolderLock, SquareTerminal, ShieldCheck, FolderUp, Globe, Laptop } from "@lucide/svelte";
  import type { Protocol, Workspace } from "$lib/api";
  import { colorOf } from "$lib/workspaces";
  import { t } from "$lib/i18n.svelte";

  let {
    protocol,
    onprotocol,
    groups,
    workspaces,
    name = $bindable(),
    host = $bindable(),
    port = $bindable(),
    user = $bindable(),
    group = $bindable(),
    workspace = $bindable(),
    remotePath = $bindable(),
    localPath = $bindable(),
  }: {
    protocol: Protocol;
    /** Switching protocol also moves the port and sign-in along, so the dialog does it. */
    onprotocol: (p: Protocol) => void;
    groups: string[];
    workspaces: Workspace[];
    name: string;
    host: string;
    port: number;
    user: string;
    group: string;
    workspace: string;
    remotePath: string;
    localPath: string;
  } = $props();

  const protocols: { id: Protocol; label: string; hint: string; icon: typeof FolderLock; ready: boolean }[] = [
    { id: "sftp", label: "SFTP", hint: "Files", icon: FolderLock, ready: true },
    { id: "ssh", label: "SSH", hint: "Terminal only", icon: SquareTerminal, ready: true },
    { id: "ftps", label: "FTPS", hint: "FTP with TLS", icon: ShieldCheck, ready: true },
    { id: "ftp", label: "FTP", hint: "Unencrypted", icon: FolderUp, ready: true },
  ];

  /** Accept pasted `sftp://user@host:port/path` or `user@host`. */
  function onHostPaste(e: ClipboardEvent) {
    const text = e.clipboardData?.getData("text")?.trim() ?? "";
    const m = text.match(/^(?:(sftp|ssh|ftps?):\/\/)?(?:([^@\s/]+)@)?([^:\s/]+)(?::(\d+))?(\/\S*)?$/);
    if (!m || (!m[1] && !m[2])) return;
    e.preventDefault();
    if (m[1]) onprotocol(m[1] as Protocol);
    if (m[2]) user = m[2];
    host = m[3];
    if (m[4]) port = Number(m[4]);
    if (m[5]) remotePath = m[5];
    if (!name) name = m[3];
  }
</script>

<div class="proto">
  {#each protocols as p (p.id)}
    <button class:on={protocol === p.id} disabled={!p.ready} onclick={() => onprotocol(p.id)}>
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

<style>
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
  .warn {
    margin: -10px 0 18px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--amber-t);
    color: var(--amber-ink);
    font-size: 12px;
    line-height: 1.45;
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
  @media (max-width: 820px) {
    .proto {
      grid-template-columns: repeat(2, 1fr);
    }
  }
</style>
