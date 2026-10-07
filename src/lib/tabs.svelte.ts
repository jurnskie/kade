import { api, type AppError, type ConflictPolicy, type Direction, type ServerProfile } from "./api";
import { edits } from "./edits.svelte";
import { store } from "./store.svelte";
import { showError } from "./toasts.svelte";

export interface Tab {
  sessionId: string;
  server: ServerProfile;
  authLabel: string;
  remotePath: string;
  localPath: string;
  hasFiles: boolean;
  hasTerminal: boolean;
  view: "files" | "terminal" | "tunnels" | "status";
  /** Mount the terminal lazily, then keep it alive across view/tab switches. */
  terminalStarted: boolean;
  localRefresh: number;
  remoteRefresh: number;
}

/** A connection that needs the user first: an unknown host key, or a password. */
export type ConnectPrompt =
  | { kind: "hostkey"; server: ServerProfile; fingerprint: string; algorithm: string; password?: string }
  | { kind: "password"; server: ServerProfile; acceptFingerprint?: string };

/** Files that already exist where a transfer is going, waiting for the user's choice. */
export interface ConflictAsk {
  names: string[];
  dest: string;
  resolve: (p: ConflictPolicy | null) => void;
}

/** The open connections, one tab each. */
class Tabs {
  list = $state<Tab[]>([]);
  activeId = $state<string | null>(null);
  /** Server id of the connection being set up. */
  connecting = $state<string | null>(null);
  /** This computer's home folder; local panes start there unless the profile links another folder. */
  localHome = $state("");
  prompt = $state<ConnectPrompt | null>(null);
  conflict = $state<ConflictAsk | null>(null);

  active = $derived(this.list.find((t) => t.sessionId === this.activeId) ?? null);
  connectedIds = $derived(new Set(this.list.map((t) => t.server.id)));

  find(sessionId: string | null | undefined) {
    return this.list.find((t) => t.sessionId === sessionId);
  }

  sessionFor = (serverId: string | null): string | null => {
    return this.list.find((t) => t.server.id === serverId)?.sessionId ?? null;
  };

  refreshAll() {
    for (const t of this.list) {
      t.localRefresh++;
      t.remoteRefresh++;
    }
  }

  setView(tab: Tab, view: Tab["view"]) {
    if (view === "files" && !tab.hasFiles) return;
    if (view !== "files" && !tab.hasTerminal) return;
    if (view === "terminal") tab.terminalStarted = true;
    tab.view = view;
  }

  private expandHome(p: string | null): string {
    if (!p) return this.localHome;
    return p.startsWith("~") ? this.localHome + p.slice(1) : p;
  }

  async connect(server: ServerProfile, password?: string, acceptFingerprint?: string) {
    this.connecting = server.id;
    try {
      const c = await api.connect(server.id, password, acceptFingerprint);
      this.list.push({
        sessionId: c.session_id,
        server,
        authLabel: c.auth_label,
        remotePath: c.home,
        localPath: this.expandHome(server.local_path),
        hasFiles: c.has_files,
        hasTerminal: c.has_terminal,
        view: c.has_terminal && (server.protocol === "ssh" || !c.has_files) ? "terminal" : "files",
        terminalStarted: c.has_terminal && (server.protocol === "ssh" || !c.has_files),
        localRefresh: 0,
        remoteRefresh: 0,
      });
      this.activeId = c.session_id;
      // Tunnels marked "start automatically" come up with the connection.
      if (c.has_terminal) {
        for (const tun of server.tunnels ?? []) {
          if (tun.auto_start) api.tunnelStart(c.session_id, tun.id).catch(showError);
        }
      }
    } catch (e) {
      const err = e as AppError;
      if (err?.kind === "host_key_unknown") {
        this.prompt = { kind: "hostkey", server, fingerprint: err.fingerprint, algorithm: err.algorithm, password };
      } else if (err?.kind === "password_required") {
        this.prompt = { kind: "password", server, acceptFingerprint };
      } else {
        showError(e);
      }
    } finally {
      this.connecting = null;
    }
  }

  /** Switch to the server's tab, or connect when there is none. */
  open = (server: ServerProfile) => {
    store.remember(server.id);
    const existing = this.list.find((t) => t.server.id === server.id);
    if (existing) this.activeId = existing.sessionId;
    else if (this.connecting !== server.id) this.connect(server);
  };

  async close(tab: Tab) {
    const i = this.list.indexOf(tab);
    this.list.splice(i, 1);
    if (this.activeId === tab.sessionId) this.activeId = this.list[Math.max(0, i - 1)]?.sessionId ?? null;
    edits.dropSession(tab.sessionId);
    await api.disconnect(tab.sessionId).catch(showError);
  }

  /** Save a profile and keep open tabs pointing at the result; errors go to the caller. */
  saveServer = async (profile: ServerProfile): Promise<ServerProfile> => {
    const saved = await api.saveServer(profile);
    store.servers = await api.listServers();
    for (const t of this.list) if (t.server.id === saved.id) t.server = saved;
    return saved;
  };

  /** Upload or download, asking first when that would overwrite something. */
  async send(tab: Tab, direction: Direction, paths: string[], destDir?: string) {
    const dest = destDir || (direction === "upload" ? tab.remotePath : tab.localPath);
    try {
      const names = await api.transferConflicts(tab.sessionId, direction, paths, dest);
      let policy: ConflictPolicy = "overwrite";
      if (names.length) {
        const choice = await new Promise<ConflictPolicy | null>((resolve) => (this.conflict = { names, dest, resolve }));
        this.conflict = null;
        if (!choice) return;
        policy = choice;
      }
      await api.transferStart(tab.sessionId, direction, paths, dest, policy);
    } catch (e) {
      showError(e);
    }
  }
}

export const tabs = new Tabs();
