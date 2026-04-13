import { useMemo, memo } from "react";
import type { SubProcess } from "../types";
import { TerminalView } from "./TerminalView";

const COLLAPSED_HEIGHT = 32;

interface SubProcessTabsProps {
  subProcesses: SubProcess[];
  activeTabId: string | null;
  onSelectTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  height: number;
  onResizeStart: (e: React.MouseEvent) => void;
  // Terminal integration props
  isDark: boolean;
  onInput: (taskId: string, data: string) => void;
  onResize: (taskId: string, cols: number, rows: number) => void;
  onRegisterTerminal: (
    taskId: string,
    fn: ((data: string, callback?: () => void) => void) | null,
  ) => number;
  onTerminalReady: (taskId: string, generation: number) => void;
  onSnapshot: (taskId: string, snapshot: string) => void;
  getRestoreState: (taskId: string) => {
    initialData?: string;
    initialSnapshot?: string;
  };
  /** Map from subprocess id → actual task id (for terminal data routing) */
  subProcessTaskMap: Record<string, string>;
}

export function SubProcessTabs({
  subProcesses,
  activeTabId,
  onSelectTab,
  onCloseTab,
  height,
  onResizeStart,
  isDark,
  onInput,
  onResize,
  onRegisterTerminal,
  onTerminalReady,
  onSnapshot,
  getRestoreState,
  subProcessTaskMap,
}: SubProcessTabsProps) {
  if (subProcesses.length === 0) return null;

  const activeSubProcess = subProcesses.find((sp) => sp.id === activeTabId);
  const activeTaskId = activeTabId ? subProcessTaskMap[activeTabId] : null;
  const isExpanded = Boolean(activeSubProcess);

  return (
    <div style={{ ...styles.container, height: isExpanded ? height : COLLAPSED_HEIGHT }}>
      {/* Resize handle */}
      {isExpanded && <div style={styles.resizeHandle} onMouseDown={onResizeStart} />}

      {/* Tab bar */}
      <div style={styles.tabBar}>
        <div style={styles.tabBarLabel}>子进程终端</div>
        <div style={styles.tabList}>
          {subProcesses.map((sp) => (
            <SubProcessTab
              key={sp.id}
              subProcess={sp}
              isActive={sp.id === activeTabId}
              onSelect={() => onSelectTab(sp.id)}
              onClose={() => onCloseTab(sp.id)}
            />
          ))}
        </div>
      </div>

      {/* Terminal content area */}
      {activeSubProcess && (
        <div style={styles.content}>
          {activeTaskId && activeSubProcess.status === "running" ? (
            <TerminalView
              key={activeTaskId}
              onInput={(data) => onInput(activeTaskId, data)}
              onResize={(cols, rows) => onResize(activeTaskId, cols, rows)}
              onRegisterTerminal={(fn) => onRegisterTerminal(activeTaskId, fn)}
              onReady={(gen) => onTerminalReady(activeTaskId, gen)}
              onSnapshot={(snap) => onSnapshot(activeTaskId, snap)}
              isDark={isDark}
              isActive
              {...getRestoreState(activeTaskId)}
            />
          ) : (
            <div style={styles.terminalPlaceholder}>
              {activeSubProcess.status === "pending_approval" && (
                <span>⏳ 等待审批...</span>
              )}
              {activeSubProcess.status === "done" && (
                <span>✅ 子任务已完成</span>
              )}
              {activeSubProcess.status === "failed" && (
                <span>❌ 子任务失败</span>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ── Tab item ─────────────────────────────────────────────────────────────────

const SubProcessTab = memo(function SubProcessTab({
  subProcess,
  isActive,
  onSelect,
  onClose,
}: {
  subProcess: SubProcess;
  isActive: boolean;
  onSelect: () => void;
  onClose: () => void;
}) {
  const label = useMemo(() => {
    const desc = subProcess.description;
    return desc.length > 40 ? desc.slice(0, 40) + "…" : desc;
  }, [subProcess.description]);

  const icon = subProcess.agent === "claude" ? "🟣" : "🟢";
  const isRunning = subProcess.status === "running";
  const isPending = subProcess.status === "pending_approval";

  return (
    <div
      style={{
        ...styles.tab,
        ...(isActive ? styles.tabActive : {}),
      }}
      onClick={onSelect}
    >
      <span style={styles.tabIcon}>{icon}</span>
      {isRunning && <span style={styles.tabPulse} />}
      {isPending && <span style={styles.tabPendingDot} />}
      <span style={styles.tabLabel}>{label}</span>
      <StatusBadge status={subProcess.status} />
      <button
        style={styles.tabCloseBtn}
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
      >
        ×
      </button>
    </div>
  );
});

function StatusBadge({ status }: { status: SubProcess["status"] }) {
  const config = statusConfig[status];
  return (
    <span
      style={{
        ...styles.statusBadge,
        color: config.color,
        background: config.bg,
      }}
    >
      {config.label}
    </span>
  );
}

const statusConfig: Record<
  SubProcess["status"],
  { label: string; color: string; bg: string }
> = {
  pending_approval: {
    label: "待审查",
    color: "#f59e0b",
    bg: "rgba(245,158,11,0.1)",
  },
  running: { label: "运行中", color: "#22c55e", bg: "rgba(34,197,94,0.1)" },
  done: { label: "完成", color: "#8b5cf6", bg: "rgba(139,92,246,0.1)" },
  failed: { label: "失败", color: "#ef4444", bg: "rgba(239,68,68,0.1)" },
};

// ── Styles ───────────────────────────────────────────────────────────────────

const styles = {
  container: {
    display: "flex",
    flexDirection: "column" as const,
    borderTop: "1px solid var(--border-primary)",
    background: "var(--bg-primary)",
    flexShrink: 0,
    position: "relative" as const,
  },
  resizeHandle: {
    position: "absolute" as const,
    top: 0,
    left: 0,
    right: 0,
    height: "4px",
    cursor: "row-resize",
    zIndex: 10,
  },
  tabBar: {
    display: "flex",
    alignItems: "center",
    gap: "8px",
    padding: "0 8px",
    height: "32px",
    borderBottom: "1px solid var(--border-primary)",
    background: "var(--bg-secondary)",
    flexShrink: 0,
    overflowX: "auto" as const,
  },
  tabBarLabel: {
    fontSize: "10px",
    fontWeight: 600,
    color: "var(--text-tertiary)",
    textTransform: "uppercase" as const,
    letterSpacing: "0.5px",
    flexShrink: 0,
    paddingRight: "8px",
    borderRight: "1px solid var(--border-primary)",
  },
  tabList: {
    display: "flex",
    alignItems: "center",
    gap: "2px",
    overflow: "hidden",
  },
  tab: {
    display: "flex",
    alignItems: "center",
    gap: "4px",
    padding: "4px 8px",
    borderRadius: "4px",
    fontSize: "11px",
    color: "var(--text-secondary)",
    cursor: "pointer",
    whiteSpace: "nowrap" as const,
    flexShrink: 0,
    transition: "background 0.1s",
  },
  tabActive: {
    background: "var(--bg-tertiary)",
    color: "var(--text-primary)",
  },
  tabIcon: { fontSize: "10px" },
  tabLabel: {
    maxWidth: "200px",
    overflow: "hidden",
    textOverflow: "ellipsis",
  },
  tabPulse: {
    width: "6px",
    height: "6px",
    borderRadius: "50%",
    background: "#22c55e",
    animation: "pulse 1.5s ease-in-out infinite",
    flexShrink: 0,
  },
  tabPendingDot: {
    width: "6px",
    height: "6px",
    borderRadius: "50%",
    background: "#f59e0b",
    flexShrink: 0,
  },
  tabCloseBtn: {
    width: "14px",
    height: "14px",
    borderRadius: "2px",
    border: "none",
    background: "transparent",
    color: "var(--text-tertiary)",
    fontSize: "12px",
    cursor: "pointer",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    padding: 0,
    marginLeft: "2px",
    flexShrink: 0,
  },
  statusBadge: {
    fontSize: "9px",
    padding: "1px 4px",
    borderRadius: "3px",
    fontWeight: 600,
    flexShrink: 0,
  },
  content: {
    flex: 1,
    overflow: "hidden",
    position: "relative" as const,
  },
  terminalPlaceholder: {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    height: "100%",
    color: "var(--text-secondary)",
    fontSize: "13px",
    opacity: 0.6,
  },
};
