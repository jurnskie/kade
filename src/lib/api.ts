import { Channel, invoke } from "@tauri-apps/api/core";
import { t } from "./i18n.svelte";

export type Protocol = "sftp" | "ssh" | "ftps" | "ftp";

export type Auth =
  | { method: "one_password"; key_fingerprint: string | null; account?: string | null; key_item?: string | null }
  | { method: "agent"; key_fingerprint: string | null }
  | { method: "key_file"; path: string }
  | { method: "password" }
  | { method: "one_password_secret"; reference: string; account?: string | null };

export interface ServerProfile {
  id: string;
  name: string;
  protocol: Protocol;
  host: string;
  port: number;
  user: string;
  group: string;
  auth: Auth;
  remote_path: string | null;
  local_path: string | null;
  /** Workspace id; "" means the default workspace. */
  workspace: string;
  tunnels: Tunnel[];
  updated_at: number;
}

/** Local port forward over SSH: localhost:local_port → remote_host:remote_port on the server. */
export interface Tunnel {
  id: string;
  name: string;
  local_port: number;
  remote_host: string;
  remote_port: number;
  auto_start: boolean;
}

export interface TunnelState {
  tunnel_id: string;
  local_port: number;
  /** Connections carried right now. */
  open: number;
  /** Connections carried since it started. */
  total: number;
}

export interface ServerStatus {
  hostname: string | null;
  os: string | null;
  kernel: string | null;
  uptime_secs: number | null;
  load: [number, number, number] | null;
  cpus: number | null;
  cpu_percent: number | null;
  mem_total: number | null;
  mem_available: number | null;
  swap_total: number | null;
  swap_free: number | null;
  disks: { mount: string; filesystem: string; size: number; used: number; available: number }[];
  processes: { pid: number; cpu: number; mem: number; command: string }[];
}

export interface Workspace {
  id: string;
  name: string;
  color: string;
  op_account: string | null;
  updated_at: number;
}

export interface Settings {
  show_hidden: boolean;
  backup_retention_days: number;
  updated_at: number;
}

export type Side = "local" | "remote";

export interface UpdateInfo {
  current: string;
  latest: string | null;
  newer: boolean;
  notes: string | null;
  install: { kind: "app_image" | "mac_app"; path: string } | { kind: "other"; reason: string };
  error: string | null;
}

export interface McpStatus {
  enabled: boolean;
  running: boolean;
  url: string;
  token: string;
  error: string | null;
}

export interface EditorOption {
  label: string;
  command: string;
  terminal: boolean;
}

export interface EditorChoice {
  current: string;
  automatic: string;
  options: EditorOption[];
}

export type EditState = "watching" | "uploading" | "uploaded" | "conflict" | "error";

export interface EditInfo {
  id: string;
  session_id: string;
  name: string;
  remote_path: string;
  local_path: string;
  state: EditState;
  message: string | null;
  at: number;
  uploads: number;
}

export interface Transaction {
  id: string;
  created: number;
  side: Side;
  op: "delete" | "overwrite" | "restore";
  summary: string;
  server_id: string | null;
  server_name: string | null;
  entries: { original: string; stored: string; stored_on: Side; is_dir: boolean }[];
  restored: boolean;
}

export type Direction = "upload" | "download";
export type ConflictPolicy = "overwrite" | "skip" | "newer";
export type JobState = "queued" | "scanning" | "running" | "paused" | "done" | "failed" | "cancelled";

export interface Progress {
  id: string;
  session_id: string;
  name: string;
  direction: Direction;
  dest: string;
  state: JobState;
  bytes_done: number;
  bytes_total: number;
  files_done: number;
  files_total: number;
  skipped: number;
  speed: number;
  error: string | null;
  backup_id: string | null;
}

export type ImportSource = "cyberduck" | "filezilla" | "transmit" | "ssh_config";

export interface ImportFound {
  source: ImportSource;
  path: string;
}

export interface ImportPreview {
  candidates: { profile: ServerProfile; duplicate: boolean }[];
  skipped: string[];
}

export interface SyncStatus {
  sync_dir: string | null;
  data_file: string;
  merged_conflicts: string[];
}

export interface Entry {
  name: string;
  path: string;
  is_dir: boolean;
  is_symlink: boolean;
  size: number;
  modified: number | null;
  permissions: string | null;
}

export interface AgentKey {
  comment: string;
  fingerprint: string;
  algorithm: string;
}

export interface Connected {
  session_id: string;
  home: string;
  auth_label: string;
  has_files: boolean;
  has_terminal: boolean;
}

export interface OpAccount {
  id: string;
  email: string;
  url: string;
}

export interface OpVault {
  id: string;
  name: string;
}

export interface OpSshKey {
  title: string;
  vault: string;
  vault_id: string;
  fingerprint: string;
  item: string;
}

export interface OpItem {
  title: string;
  vault: string;
  vault_id: string;
  username: string | null;
  url: string | null;
  reference: string;
}

export type TermEvent = { type: "data"; bytes: number[] } | { type: "exit" };

/** Mirrors `AppError` in src-tauri/src/error.rs. */
export type AppError =
  | { kind: "host_key_unknown"; fingerprint: string; algorithm: string }
  | { kind: "host_key_changed"; line: number }
  | { kind: "auth_failed"; message: string }
  | { kind: "password_required" }
  | { kind: "agent_unavailable"; message: string }
  | { kind: "session_not_found" }
  | { kind: "unsupported"; message: string }
  | { kind: "other"; message: string };

export function errorMessage(e: unknown): string {
  const err = e as AppError;
  switch (err?.kind) {
    case "host_key_unknown":
      return t("Unknown host key ({fp})", { fp: err.fingerprint });
    case "host_key_changed":
      return t("The host key changed (known_hosts line {line}). This may be an attack — check the server.", { line: err.line });
    case "auth_failed":
      return t("Login failed: {m}", { m: err.message });
    case "password_required":
      return t("Password required");
    case "agent_unavailable":
      return t("Agent not reachable: {m}", { m: err.message });
    case "session_not_found":
      return t("The connection was closed");
    case "unsupported":
    case "other":
      return err.message;
    default:
      return String(e);
  }
}

/** File operations a pane can perform; local and remote implement the same shape. */
export interface FileOps {
  mkdir: (path: string) => Promise<void>;
  createFile: (path: string) => Promise<void>;
  rename: (from: string, to: string) => Promise<void>;
  /** Moves items into a backup transaction; returns it for "undo". */
  remove: (paths: string[]) => Promise<Transaction | null>;
}

export function joinPath(dir: string, name: string): string {
  return dir.endsWith("/") ? dir + name : `${dir}/${name}`;
}

export const localOps: FileOps = {
  mkdir: (path) => invoke("local_mkdir", { path }),
  createFile: (path) => invoke("local_create_file", { path }),
  rename: (from, to) => invoke("local_rename", { from, to }),
  remove: (paths) => invoke("local_delete", { paths }),
};

export function remoteOps(sessionId: string): FileOps {
  return {
    mkdir: (path) => invoke("remote_mkdir", { sessionId, path }),
    createFile: (path) => invoke("remote_create_file", { sessionId, path }),
    rename: (from, to) => invoke("remote_rename", { sessionId, from, to }),
    remove: (paths) => invoke("remote_delete", { sessionId, paths }),
  };
}

export const api = {
  listServers: () => invoke<ServerProfile[]>("list_servers"),
  saveServer: (profile: ServerProfile) => invoke<ServerProfile>("save_server", { profile }),
  importDetect: () => invoke<ImportFound[]>("import_detect"),
  importPreview: (source: ImportSource, path: string) => invoke<ImportPreview>("import_preview", { source, path }),
  importApply: (profiles: ServerProfile[], workspace: string) => invoke<number>("import_apply", { profiles, workspace }),
  deleteServer: (id: string) => invoke<void>("delete_server", { id }),
  backupsList: () => invoke<Transaction[]>("backups_list"),
  backupRestore: (id: string, sessionId: string | null) =>
    invoke<Transaction | null>("backup_restore", { id, sessionId }),
  backupDelete: (id: string, sessionId: string | null) => invoke<void>("backup_delete", { id, sessionId }),
  transferConflicts: (sessionId: string, direction: Direction, sources: string[], destDir: string) =>
    invoke<string[]>("transfer_conflicts", { sessionId, direction, sources, destDir }),
  transferStart: (
    sessionId: string,
    direction: Direction,
    sources: string[],
    destDir: string,
    conflict: ConflictPolicy,
  ) => invoke<string[]>("transfer_start", { sessionId, direction, sources, destDir, conflict }),
  transferPause: (id: string, paused: boolean) => invoke<void>("transfer_pause", { id, paused }),
  transferCancel: (id: string) => invoke<void>("transfer_cancel", { id }),
  listWorkspaces: () => invoke<Workspace[]>("list_workspaces"),
  saveWorkspace: (workspace: Workspace) => invoke<Workspace>("save_workspace", { workspace }),
  deleteWorkspace: (id: string, moveTo: string) => invoke<void>("delete_workspace", { id, moveTo }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  syncStatus: () => invoke<SyncStatus>("sync_status"),
  setSyncDir: (dir: string | null) => invoke<SyncStatus>("set_sync_dir", { dir }),
  editOpen: (sessionId: string, path: string) => invoke<EditInfo>("edit_open", { sessionId, path }),
  editForce: (id: string) => invoke<void>("edit_force", { id }),
  editReload: (id: string) => invoke<void>("edit_reload", { id }),
  editStop: (id: string) => invoke<void>("edit_stop", { id }),
  editsList: () => invoke<EditInfo[]>("edits_list"),
  /** Per-machine editor command; empty = Omarchy's editor or the system default. */
  getEditor: () => invoke<EditorChoice>("get_editor"),
  setEditor: (editor: string) => invoke<void>("set_editor", { editor }),
  openLocal: (path: string) => invoke<void>("open_local", { path }),
  updateCheck: () => invoke<UpdateInfo>("update_check"),
  updateInstall: (version: string) => invoke<void>("update_install", { version }),
  mcpStatus: () => invoke<McpStatus>("mcp_status"),
  mcpSetEnabled: (enabled: boolean) => invoke<McpStatus>("mcp_set_enabled", { enabled }),
  mcpRegenerateToken: () => invoke<McpStatus>("mcp_regenerate_token"),
  opAccounts: () => invoke<OpAccount[]>("op_accounts"),
  opVaults: (account: string | null) => invoke<OpVault[]>("op_vaults", { account }),
  opItems: (account: string | null, vault: string | null) => invoke<OpItem[]>("op_items", { account, vault }),
  opSshKeys: (account: string | null, vault: string | null) =>
    invoke<OpSshKey[]>("op_ssh_keys", { account, vault }),
  agentKeys: (auth: Auth) => invoke<AgentKey[]>("agent_keys", { auth }),
  connect: (serverId: string, password?: string, acceptFingerprint?: string) =>
    invoke<Connected>("connect", {
      serverId,
      password: password ?? null,
      acceptFingerprint: acceptFingerprint ?? null,
    }),
  disconnect: (sessionId: string) => invoke<void>("disconnect", { sessionId }),
  remoteList: (sessionId: string, path: string) =>
    invoke<Entry[]>("remote_list", { sessionId, path }),
  terminalOpen: (sessionId: string, cols: number, rows: number, onEvent: (e: TermEvent) => void) => {
    const events = new Channel<TermEvent>();
    events.onmessage = onEvent;
    return invoke<string>("terminal_open", { sessionId, cols, rows, events });
  },
  terminalWrite: (termId: string, data: string) => invoke<void>("terminal_write", { termId, data }),
  terminalResize: (termId: string, cols: number, rows: number) =>
    invoke<void>("terminal_resize", { termId, cols, rows }),
  terminalClose: (termId: string) => invoke<void>("terminal_close", { termId }),
  localHome: () => invoke<string>("local_home"),
  localList: (path: string) => invoke<Entry[]>("local_list", { path }),
  serverStatus: (sessionId: string) => invoke<ServerStatus>("server_status", { sessionId }),
  tunnelStart: (sessionId: string, tunnelId: string) => invoke<TunnelState>("tunnel_start", { sessionId, tunnelId }),
  tunnelStop: (sessionId: string, tunnelId: string) => invoke<void>("tunnel_stop", { sessionId, tunnelId }),
  tunnelsList: (sessionId: string) => invoke<TunnelState[]>("tunnels_list", { sessionId }),
};
