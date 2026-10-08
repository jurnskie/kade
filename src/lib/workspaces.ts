import type { ServerProfile, Workspace } from "./api";

export const DEFAULT_WORKSPACE = "default";

/** Workspace colours. The dots are CSS tokens (--ws-*, --on-ws-*) with a light
 *  and a dark variant in themes.css. `tint` (backgrounds) and `ink` (text) are
 *  mixed with the current theme's surface and text colours, so they work in
 *  every palette. `onColor` is the text colour on top of a `color` fill. */
const swatch = (key: string, label: string) => ({
  label,
  color: `var(--ws-${key})`,
  onColor: `var(--on-ws-${key})`,
  tint: `color-mix(in srgb, var(--ws-${key}) 16%, var(--paper))`,
  ink: `color-mix(in srgb, var(--ws-${key}) 78%, var(--granite))`,
});
export const WORKSPACE_COLORS: Record<string, { label: string; color: string; onColor: string; tint: string; ink: string }> = {
  pine: swatch("pine", "Pine"),
  blue: swatch("blue", "Fjord"),
  amber: swatch("amber", "Amber"),
  plum: swatch("plum", "Plum"),
  coral: swatch("coral", "Coral"),
  slate: swatch("slate", "Slate"),
};

export const colorOf = (ws: Workspace | null | undefined) => WORKSPACE_COLORS[ws?.color ?? "pine"] ?? WORKSPACE_COLORS.pine;

export const workspaceIdOf = (s: ServerProfile) => s.workspace || DEFAULT_WORKSPACE;
