import type { ITheme } from "@xterm/xterm";

const ANSI = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;

/** The xterm theme for the active palette, read from the --term-* tokens in
 *  themes.css. Call after the data-palette attribute changed. */
export function termTheme(): ITheme {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string) => css.getPropertyValue(`--term-${name}`).trim();
  const theme: Record<string, string> = {
    background: v("bg"),
    foreground: v("fg"),
    cursor: v("cursor"),
    cursorAccent: v("bg"),
    selectionBackground: v("sel"),
  };
  ANSI.forEach((name, i) => {
    theme[name] = v(String(i));
    theme[`bright${name[0].toUpperCase()}${name.slice(1)}`] = v(String(i + 8));
  });
  return theme;
}
