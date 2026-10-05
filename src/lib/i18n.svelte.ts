import { invoke } from "@tauri-apps/api/core";
import nl from "./locales/nl";

export type Lang = "en" | "nl";
export type LangChoice = Lang | "auto";

const KEY = "kade.language";

function systemLang(): Lang {
  return (navigator.language || "en").toLowerCase().startsWith("nl") ? "nl" : "en";
}

function storedChoice(): LangChoice {
  try {
    const v = localStorage.getItem(KEY);
    return v === "en" || v === "nl" ? v : "auto";
  } catch {
    return "auto";
  }
}

class I18n {
  choice = $state<LangChoice>(storedChoice());
  lang = $derived<Lang>(this.choice === "auto" ? systemLang() : this.choice);

  /** Change the language (per machine); the backend follows for its messages. */
  set(choice: LangChoice) {
    this.choice = choice;
    try {
      localStorage.setItem(KEY, choice);
    } catch {
      /* not persisted; fine */
    }
    void invoke("set_language", { language: this.lang, remember: choice }).catch(() => {});
  }
}

export const i18n = new I18n();

/** Tell the backend the effective language once at startup. */
export function syncBackendLanguage() {
  void invoke("set_language", { language: i18n.lang, remember: null }).catch(() => {});
}

/**
 * Translate English source text. `{name}` placeholders are filled from
 * `params`. Missing Dutch translations fall back to English.
 */
export function t(en: string, params?: Record<string, string | number>): string {
  const text = i18n.lang === "nl" ? (nl[en] ?? en) : en;
  return params ? text.replace(/\{(\w+)\}/g, (m, k) => (k in params ? String(params[k]) : m)) : text;
}

/** Pick the singular or plural English form, then translate it. */
export function tn(n: number, one: string, many: string, params?: Record<string, string | number>): string {
  return t(n === 1 ? one : many, { n, ...params });
}

/** BCP 47 locale for dates and numbers. */
export function locale(): string {
  return i18n.lang === "nl" ? "nl-NL" : "en-GB";
}
