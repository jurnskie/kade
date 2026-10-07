import { api, type ServerProfile, type Settings, type Workspace } from "./api";
import { workspaceIdOf } from "./workspaces";

// Workspaces are synced; which one is active, and what was opened recently,
// is remembered per machine (a convenience; storage may fail).
const WS_KEY = "kade.workspace";
const RECENT_KEY = "kade.recent";

function stored<T>(key: string, parse: (raw: string | null) => T, fallback: T): T {
  try {
    return parse(localStorage.getItem(key));
  } catch {
    return fallback;
  }
}

function persist(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* not persisted; fine */
  }
}

/** Connections, workspaces and settings from kade.json, plus this machine's picks. */
class Store {
  servers = $state<ServerProfile[]>([]);
  workspaces = $state<Workspace[]>([]);
  settings = $state<Settings>({ show_hidden: false, backup_retention_days: 7, updated_at: 0 });
  activeWsId = $state(stored(WS_KEY, (v) => v ?? "", ""));
  recent = $state<string[]>(stored(RECENT_KEY, (v) => JSON.parse(v ?? "[]"), []));

  activeWorkspace = $derived(this.workspaces.find((w) => w.id === this.activeWsId) ?? this.workspaces[0] ?? null);
  visibleServers = $derived(this.servers.filter((s) => workspaceIdOf(s) === this.activeWorkspace?.id));
  /** Connections per workspace id. */
  counts = $derived(
    this.servers.reduce<Record<string, number>>((acc, s) => {
      const id = workspaceIdOf(s);
      acc[id] = (acc[id] ?? 0) + 1;
      return acc;
    }, {}),
  );
  groups = $derived([...new Set(this.visibleServers.map((s) => s.group).filter(Boolean))]);

  async reload() {
    this.workspaces = await api.listWorkspaces();
    if (!this.workspaces.some((w) => w.id === this.activeWsId)) this.activeWsId = this.workspaces[0]?.id ?? "";
    this.servers = await api.listServers();
    this.settings = await api.getSettings();
  }

  switchWorkspace(id: string) {
    this.activeWsId = id;
    persist(WS_KEY, id);
  }

  remember(serverId: string) {
    this.recent = [serverId, ...this.recent.filter((r) => r !== serverId)].slice(0, 10);
    persist(RECENT_KEY, JSON.stringify(this.recent));
  }
}

export const store = new Store();
