import type { ServerProfile, Workspace } from "./api";

export const DEFAULT_WORKSPACE = "default";

/** Workspace colours, each with a soft tint for backgrounds. */
export const WORKSPACE_COLORS: Record<string, { label: string; color: string; tint: string }> = {
  pine: { label: "Pine", color: "#1d6b57", tint: "#e3efea" },
  blue: { label: "Fjord", color: "#2f6fb0", tint: "#e4edf7" },
  amber: { label: "Amber", color: "#b7791f", tint: "#f6eedf" },
  plum: { label: "Plum", color: "#8a4f8f", tint: "#f2e8f3" },
  coral: { label: "Coral", color: "#c2553f", tint: "#f8e6e2" },
  slate: { label: "Slate", color: "#4a524e", tint: "#eceeed" },
};

export const colorOf = (ws: Workspace | null | undefined) => WORKSPACE_COLORS[ws?.color ?? "pine"] ?? WORKSPACE_COLORS.pine;

export const workspaceIdOf = (s: ServerProfile) => s.workspace || DEFAULT_WORKSPACE;
