/** Shortcut hints follow the platform: handlers accept Cmd or Ctrl, but labels show what the user has. */
export const isMac = typeof navigator !== "undefined" && /mac/i.test(navigator.platform || navigator.userAgent);
export const modKey = isMac ? "⌘" : "Ctrl";
