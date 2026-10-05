import { listen } from "@tauri-apps/api/event";
import { api, type EditInfo } from "./api";

/** Files currently open in the user's editor, kept in sync with the backend. */
class OpenEdits {
  items = $state<EditInfo[]>([]);
  /** Called after each successful upload, e.g. to refresh the server pane. */
  onUploaded: ((e: EditInfo) => void) | null = null;

  constructor() {
    listen<EditInfo>("edit", ({ payload }) => this.update(payload));
    api.editsList().then((list) => (this.items = list)).catch(() => {});
  }

  private update(e: EditInfo) {
    const i = this.items.findIndex((x) => x.id === e.id);
    const before = i >= 0 ? this.items[i].uploads : 0;
    if (i >= 0) this.items[i] = e;
    else this.items.push(e);
    if (e.uploads > before) this.onUploaded?.(e);
  }

  async stop(id: string) {
    await api.editStop(id);
    this.items = this.items.filter((x) => x.id !== id);
  }

  dropSession(sessionId: string) {
    this.items = this.items.filter((x) => x.session_id !== sessionId);
  }
}

export const edits = new OpenEdits();
