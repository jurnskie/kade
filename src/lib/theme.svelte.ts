/** Two independent choices, both remembered per machine and applied to <html>:
 *  - mode: light, dark or follow the system  -> data-scheme (resolved to light|dark)
 *  - palette: pine, haven, duin or schemer   -> data-palette
 *  themes.css reads only those two attributes. app.html applies the stored
 *  choices before the app loads, so there's no flash of the wrong theme. */
export type ThemeChoice = "auto" | "light" | "dark";
export type Scheme = "light" | "dark";

/** Palette ids. Keep in sync with the allow-list in app.html and themes.css. */
export const PALETTES = [
  { id: "pine", name: "Pine" },
  { id: "haven", name: "Haven" },
  { id: "duin", name: "Duin" },
  { id: "schemer", name: "Schemer" },
] as const;
export type PaletteId = (typeof PALETTES)[number]["id"];

const KEY = "kade.theme";
const PALETTE_KEY = "kade.palette";

function stored(): ThemeChoice {
  try {
    const v = localStorage.getItem(KEY);
    return v === "light" || v === "dark" ? v : "auto";
  } catch {
    return "auto";
  }
}

function storedPalette(): PaletteId {
  try {
    const v = localStorage.getItem(PALETTE_KEY);
    return PALETTES.find((p) => p.id === v)?.id ?? "pine";
  } catch {
    return "pine";
  }
}

const dark = window.matchMedia("(prefers-color-scheme: dark)");

const resolve = (choice: ThemeChoice): Scheme => (choice === "auto" ? (dark.matches ? "dark" : "light") : choice);

/** Synchronous on purpose: the terminal reads the resulting CSS right after. */
export function applyTheme(choice: ThemeChoice, palette: PaletteId) {
  const root = document.documentElement;
  root.dataset.scheme = resolve(choice);
  root.dataset.palette = palette;
}

class Theme {
  choice = $state<ThemeChoice>(stored());
  palette = $state<PaletteId>(storedPalette());
  /** The scheme in effect: `choice`, or the system's while it's "auto". */
  scheme = $state<Scheme>(resolve(this.choice));

  constructor() {
    applyTheme(this.choice, this.palette);
    dark.addEventListener("change", () => {
      if (this.choice === "auto") this.refresh();
    });
  }

  private refresh() {
    applyTheme(this.choice, this.palette);
    this.scheme = resolve(this.choice);
  }

  set(choice: ThemeChoice) {
    this.choice = choice;
    this.refresh();
    remember(KEY, choice);
  }

  setPalette(palette: PaletteId) {
    this.palette = palette;
    this.refresh();
    remember(PALETTE_KEY, palette);
  }
}

function remember(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* not persisted; fine */
  }
}

export const theme = new Theme();
