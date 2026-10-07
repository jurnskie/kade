import { locale, t } from "./i18n.svelte";

/** One formatter per locale and kind; building an Intl formatter is slow and file lists call these per row. */
function cached<T>(make: (locale: string) => T): () => T {
  const byLocale = new Map<string, T>();
  return () => {
    const loc = locale();
    let f = byLocale.get(loc);
    if (!f) byLocale.set(loc, (f = make(loc)));
    return f;
  };
}

const sizeFmt = cached((l) => new Intl.NumberFormat(l, { maximumFractionDigits: 1 }));

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${sizeFmt().format(value)} ${units[unit]}`;
}

const timeFmt = cached((l) => new Intl.DateTimeFormat(l, { hour: "2-digit", minute: "2-digit" }));
const dayFmt = cached((l) => new Intl.DateTimeFormat(l, { day: "numeric", month: "short" }));
const yearFmt = cached((l) => new Intl.DateTimeFormat(l, { day: "numeric", month: "short", year: "numeric" }));

/** Locale-aware string comparison, as `localeCompare` but without per-call setup. */
export const collator = cached((l) => new Intl.Collator(l));

export function formatDate(unixSeconds: number | null): string {
  if (unixSeconds == null) return "—";
  const d = new Date(unixSeconds * 1000);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return t("Today, {time}", { time: timeFmt().format(d) });
  if (d.getFullYear() === now.getFullYear()) return `${dayFmt().format(d)}, ${timeFmt().format(d)}`;
  return yearFmt().format(d);
}

/** Parent of a POSIX path; "/" stays "/". */
export function parentPath(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const idx = trimmed.lastIndexOf("/");
  return idx <= 0 ? "/" : trimmed.slice(0, idx);
}

/** Show paths under the home directory as `~/…`. */
export function tildify(path: string, home: string): string {
  if (home && (path === home || path.startsWith(home + "/"))) return "~" + path.slice(home.length);
  return path;
}
