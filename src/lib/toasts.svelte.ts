import { errorMessage, type Transaction } from "./api";

/** The messages at the bottom of the window: errors, short notices, an undo offer and a waiting update. */
class Toasts {
  error = $state<string | null>(null);
  /** Follow-up offered on the error toast, e.g. retrying a failed connect. Such a toast stays until dismissed. */
  errorAction = $state<{ label: string; run: () => void } | null>(null);
  /** A short success message, e.g. after an import. */
  notice = $state<string | null>(null);
  undo = $state<{ tx: Transaction; sessionId: string | null } | null>(null);
  /** Version of a newer release that can be installed. */
  update = $state<string | null>(null);

  private errorTimer: ReturnType<typeof setTimeout> | undefined;
  private noticeTimer: ReturnType<typeof setTimeout> | undefined;
  private undoTimer: ReturnType<typeof setTimeout> | undefined;

  /** An arrow function, so it can be handed out as an `onerror` callback. */
  showError = (e: unknown, action?: { label: string; run: () => void }) => {
    clearTimeout(this.errorTimer);
    this.error = errorMessage(e);
    this.errorAction = action ?? null;
    if (!action) this.errorTimer = setTimeout(() => (this.error = null), 6000);
  };

  dismissError() {
    clearTimeout(this.errorTimer);
    this.error = null;
    this.errorAction = null;
  }

  showNotice(text: string) {
    this.notice = text;
    clearTimeout(this.noticeTimer);
    this.noticeTimer = setTimeout(() => (this.notice = null), 5000);
  }

  offerUndo(tx: Transaction, sessionId: string | null) {
    clearTimeout(this.undoTimer);
    this.undo = { tx, sessionId };
    this.undoTimer = setTimeout(() => (this.undo = null), 10_000);
  }
}

export const toasts = new Toasts();
export const showError = toasts.showError;
