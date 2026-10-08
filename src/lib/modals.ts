/** Open modals, oldest first. Only the topmost one reacts to Escape and Tab. */
const stack: symbol[] = [];

/** Register a modal; call the returned function when it goes away. */
export function pushModal(): { id: symbol; pop: () => void } {
  const id = Symbol("modal");
  stack.push(id);
  return {
    id,
    pop: () => {
      const i = stack.indexOf(id);
      if (i !== -1) stack.splice(i, 1);
    },
  };
}

export const isTopModal = (id: symbol) => stack.at(-1) === id;

/** True while any modal or the quick switcher is open. */
export const modalOpen = () => stack.length > 0;

/** The one open context menu, app-wide: opening another closes it; Escape closes it. */
let menuOwner: { close: () => void } | null = null;

function onMenuKey(e: KeyboardEvent) {
  if (e.key !== "Escape" || !menuOwner) return;
  e.preventDefault();
  e.stopPropagation();
  menuOwner.close();
}

/** Register an open context menu (closing any other); call the returned function when it closes. */
export function claimMenu(close: () => void): () => void {
  const me = { close };
  const prev = menuOwner;
  menuOwner = me;
  if (!prev) window.addEventListener("keydown", onMenuKey, true);
  prev?.close();
  return () => {
    if (menuOwner !== me) return;
    menuOwner = null;
    window.removeEventListener("keydown", onMenuKey, true);
  };
}
