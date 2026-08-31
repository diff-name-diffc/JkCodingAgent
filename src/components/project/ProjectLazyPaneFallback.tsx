export function ProjectLazyPaneFallback({ label = "加载中..." }: { label?: string }) {
  return (
    <div
      style={{
        flex: 1,
        minWidth: 0,
        minHeight: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: "var(--text-muted)",
        fontSize: 13,
        background: "var(--bg-panel)",
      }}
    >
      {label}
    </div>
  );
}
