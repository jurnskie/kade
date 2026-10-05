import type { ServerProfile, Workspace } from "./api";

export const DEFAULT_WORKSPACE = "default";

/** Workspace colours. `tint` (backgrounds) and `ink` (text) are mixed with the
 *  current theme's surface and text colours, so they work in light and dark. */
const swatch = (label: string, color: string) => ({
  label,
  color,
  tint: `color-mix(in srgb, ${color} 16%, var(--paper))`,
  ink: `color-mix(in srgb, ${color} 72%, var(--granite))`,
});
export const WORKSPACE_COLORS: Record<string, { label: string; color: string; tint: string; ink: string }> = {
  pine: swatch("Pine", "#1d6b57"),
  blue: swatch("Fjord", "#2f6fb0"),
  amber: swatch("Amber", "#b7791f"),
  plum: swatch("Plum", "#8a4f8f"),
  coral: swatch("Coral", "#c2553f"),
  slate: swatch("Slate", "#4a524e"),
};

export const colorOf = (ws: Workspace | null | undefined) => WORKSPACE_COLORS[ws?.color ?? "pine"] ?? WORKSPACE_COLORS.pine;

export const workspaceIdOf = (s: ServerProfile) => s.workspace || DEFAULT_WORKSPACE;
