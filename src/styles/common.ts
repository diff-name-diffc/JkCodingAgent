import type React from "react";

export const common = {
  errorBoundaryWrap: {
    flex: 1,
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    justifyContent: "center",
    padding: "24px 32px",
    gap: 12,
    color: "var(--text-muted)",
    fontSize: 13,
    textAlign: "center" as const,
  },
  errorBoundaryIcon: { fontSize: 28, lineHeight: 1 },
  errorBoundaryTitle: { fontWeight: 600, color: "var(--text-secondary)" },
  errorBoundaryMessage: {
    maxWidth: 320,
    fontSize: 12,
    color: "var(--text-hint)",
    wordBreak: "break-word" as const,
    lineHeight: 1.5,
  },
  errorBoundaryActions: { display: "flex", gap: 8 },
  errorBoundaryBtn: {
    padding: "5px 16px",
    background: "var(--bg-hover)",
    border: "1px solid var(--border-dim)",
    borderRadius: 6,
    color: "var(--text-secondary)",
    fontSize: 12,
    cursor: "pointer",
    marginTop: 4,
  },
} satisfies Record<string, React.CSSProperties>;
