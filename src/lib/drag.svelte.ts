import type { Side } from "./api";

export interface DropTarget {
  side: Side;
  sessionId: string;
  dir: string;
}

interface DragSource {
  side: Side;
  sessionId: string;
  paths: string[];
}

/**
 * Pointer-based drag between panes. HTML5 drag-and-drop competes with the
 * native file-drop handler in some webviews, so this tracks the pointer
 * itself and resolves the drop target from `data-drop-*` attributes.
 */
class Drag {
  source = $state<DragSource | null>(null);
  x = $state(0);
  y = $state(0);
  over = $state<DropTarget | null>(null);
  onDrop: ((from: DragSource, to: DropTarget) => void) | null = null;

  /** Call on pointerdown on a row; the drag only starts after a few pixels. */
  arm(e: PointerEvent, source: () => DragSource) {
    if (e.button !== 0) return;
    const startX = e.clientX;
    const startY = e.clientY;

    const move = (ev: PointerEvent) => {
      if (!this.source && Math.hypot(ev.clientX - startX, ev.clientY - startY) < 6) return;
      if (!this.source) this.source = source();
      this.x = ev.clientX;
      this.y = ev.clientY;
      // Rows read `over`, so only replace it when the target actually changed.
      const next = targetAt(ev.clientX, ev.clientY);
      const cur = this.over;
      if (next?.side !== cur?.side || next?.sessionId !== cur?.sessionId || next?.dir !== cur?.dir) this.over = next;
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const from = this.source;
      const to = this.over;
      this.source = null;
      this.over = null;
      if (from && to && !(to.side === from.side && to.sessionId === from.sessionId)) this.onDrop?.(from, to);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }
}

/** Resolve the pane (and folder row, if any) under a point. */
export function targetAt(x: number, y: number): DropTarget | null {
  const el = document.elementFromPoint(x, y) as HTMLElement | null;
  const pane = el?.closest<HTMLElement>("[data-drop-side]");
  if (!pane) return null;
  const row = el?.closest<HTMLElement>("[data-drop-dir]");
  return {
    side: pane.dataset.dropSide as Side,
    sessionId: pane.dataset.dropSession ?? "",
    dir: row?.dataset.dropDir ?? pane.dataset.dropPath ?? "",
  };
}

export const drag = new Drag();
