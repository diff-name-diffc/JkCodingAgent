import type React from "react";

export const dialogs = {
  settingsBody: { flex: 1, overflowY: "auto" as const, padding: "18px 20px" },
} satisfies Record<string, React.CSSProperties>;
