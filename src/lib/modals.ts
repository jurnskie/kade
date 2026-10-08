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
