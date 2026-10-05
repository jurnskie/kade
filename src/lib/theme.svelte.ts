/** Light, dark or follow the system; remembered per machine. The choice is
 *  applied as <html data-theme>, which styles.css reads. app.html applies the
 *  stored choice before the app loads, so there's no flash of the wrong theme. */
export type ThemeChoice = "auto" | "light" | "dark";

const KEY = "kade.theme";

function stored(): ThemeChoice {
  try {
    const v = localStorage.getItem(KEY);
    return v === "light" || v === "dark" ? v : "auto";
  } catch {
    return "auto";
  }
}

export function applyTheme(choice: ThemeChoice) {
  if (choice === "auto") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = choice;
}

class Theme {
  choice = $state<ThemeChoice>(stored());

  set(choice: ThemeChoice) {
    this.choice = choice;
    applyTheme(choice);
    try {
      localStorage.setItem(KEY, choice);
    } catch {
      /* not persisted; fine */
    }
  }
}

export const theme = new Theme();
