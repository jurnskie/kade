import { listen } from "@tauri-apps/api/event";
import type { Progress } from "./api";

const FINISHED = new Set(["done", "failed", "cancelled"]);

/** Live view of the transfer queue, fed by the backend's `transfer` events. */
class TransferQueue {
  jobs = $state<Progress[]>([]);
  /** Called once per job when it ends, e.g. to refresh the destination pane. */
  onFinished: ((job: Progress) => void) | null = null;

  constructor() {
    listen<Progress>("transfer", ({ payload }) => this.update(payload));
  }

  private update(p: Progress) {
    const i = this.jobs.findIndex((j) => j.id === p.id);
    const wasFinished = i >= 0 && FINISHED.has(this.jobs[i].state);
    if (i >= 0) this.jobs[i] = p;
    else this.jobs.push(p);
    if (!wasFinished && FINISHED.has(p.state)) this.onFinished?.(p);
  }

  get active() {
    return this.jobs.filter((j) => !FINISHED.has(j.state));
  }

  clearFinished() {
    this.jobs = this.jobs.filter((j) => !FINISHED.has(j.state));
  }
}

export const transfers = new TransferQueue();
export const isFinished = (p: Progress) => FINISHED.has(p.state);
