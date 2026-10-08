/** Arrow keys move between the `role="radio"` options of a group, selecting as they go. */
export function radioGroup(node: HTMLElement) {
  function onkeydown(e: KeyboardEvent) {
    const step = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[e.key];
    if (!step) return;
    const radios = [...node.querySelectorAll<HTMLElement>('[role="radio"]:not(:disabled)')];
    const at = radios.indexOf(document.activeElement as HTMLElement);
    if (at < 0) return;
    e.preventDefault();
    const next = radios[(at + step + radios.length) % radios.length];
    next.focus();
    next.click();
  }
  node.addEventListener("keydown", onkeydown);
  return { destroy: () => node.removeEventListener("keydown", onkeydown) };
}
