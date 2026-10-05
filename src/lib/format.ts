import { locale, t } from "./i18n.svelte";

const sizeFmt = () => new Intl.NumberFormat(locale(), { maximumFractionDigits: 1 });

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

const timeFmt = () => new Intl.DateTimeFormat(locale(), { hour: "2-digit", minute: "2-digit" });
const dayFmt = () => new Intl.DateTimeFormat(locale(), { day: "numeric", month: "short" });
const yearFmt = () => new Intl.DateTimeFormat(locale(), { day: "numeric", month: "short", year: "numeric" });

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
