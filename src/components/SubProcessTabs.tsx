import { useMemo, memo } from "react";
import type { SubProcess } from "../types";
import { TerminalView } from "./TerminalView";

const COLLAPSED_HEIGHT = 32;

interface SubProcessTabsProps {
  subProcesses: SubProcess[];
  activeSessionId: string | null;
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
}

export function SubProcessTabs({
  subProcesses,
  activeSessionId,
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
}: SubProcessTabsProps) {
  const visibleSubProcesses = useMemo(
    () =>
      activeSessionId
        ? subProcesses.filter((subProcess) => subProcess.sessionId === activeSessionId)
        : [],
    [activeSessionId, subProcesses],
  );
  const mountedTerminals = useMemo(
    () => subProcesses.filter((subProcess) => subProcess.status !== "pending_approval"),
    [subProcesses],
  );
  const activeSubProcess = visibleSubProcesses.find((sp) => sp.id === activeTabId) ?? null;
  const isExpanded = Boolean(activeSubProcess);
  const isPanelVisible = visibleSubProcesses.length > 0;

  if (subProcesses.length === 0) return null;

  return (
    <div
      className={isExpanded ? "ai-subprocess-dock is-expanded" : "ai-subprocess-dock"}
      style={{
        ...styles.container,
        display: isPanelVisible ? "flex" : "none",
        height: isExpanded ? height : COLLAPSED_HEIGHT,
      }}
    >
      {/* Resize handle */}
      {isExpanded && <div className="ai-subprocess-resize" style={styles.resizeHandle} onMouseDown={onResizeStart} />}

      {/* Tab bar */}
      <div className="ai-subprocess-tabbar" style={styles.tabBar}>
        <div className="ai-subprocess-tabbar-label" style={styles.tabBarLabel}>子进程终端</div>
        <div className="ai-subprocess-tablist" style={styles.tabList}>
          {visibleSubProcesses.map((sp) => (
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
      {(activeSubProcess || mountedTerminals.length > 0) && (
        <div className="ai-subprocess-content" style={{ ...styles.content, display: activeSubProcess ? "flex" : "none" }}>
          {activeSubProcess?.status === "pending_approval" ? (
            <div className="ai-subprocess-placeholder" style={styles.terminalPlaceholder}>
              <span>⏳ 等待审批...</span>
            </div>
          ) : (
            <div className="ai-subprocess-terminal-stage" style={styles.terminalStage}>
              {mountedTerminals.map((subProcess) => {
                const taskId = subProcess.id;
                const isVisible = subProcess.id === activeTabId;
                const isInteractive = subProcess.status === "running";
                return (
                  <div
                    key={subProcess.id}
                    style={{
                      ...styles.terminalLayer,
                      display: isVisible ? "block" : "none",
                    }}
                  >
                    <div className="ai-subprocess-terminal-wrap" style={styles.terminalWrap}>
                      <TerminalView
                        onInput={(data) => {
                          if (isInteractive) {
                            onInput(taskId, data);
                          }
                        }}
                        onResize={(cols, rows) => onResize(taskId, cols, rows)}
                        onRegisterTerminal={(fn) => onRegisterTerminal(taskId, fn)}
                        onReady={(gen) => onTerminalReady(taskId, gen)}
                        onSnapshot={(snap) => onSnapshot(taskId, snap)}
                        isDark={isDark}
                        isActive={isVisible && isInteractive}
                        {...getRestoreState(taskId)}
                      />
                      {!isInteractive && isVisible && (
                        <div className="ai-subprocess-status-overlay" style={styles.terminalStatusOverlay}>
                          {subProcess.status === "done" && (
                            <span>终端已退出，可继续查看本次输入与输出</span>
                          )}
                          {subProcess.status === "failed" && (
                            <span>
                              终端已失败退出
                              {subProcess.failureReason ? `：${subProcess.failureReason}` : ""}
                            </span>
                          )}
                          {subProcess.status === "stopped" && (
                            <span>终端已停止，历史输出已保留，可稍后继续运行</span>
                          )}
                        </div>
                      )}
                    </div>
                  </div>
                );
              })}
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
  const agentLabel = subProcess.agent === "claude" ? "Claude" : "Codex";
  const isRunning = subProcess.status === "running";
  const isPending = subProcess.status === "pending_approval";

  return (
    <div
      className={isActive ? "ai-subprocess-tab is-active" : "ai-subprocess-tab"}
      style={{
        ...styles.tab,
        ...(isActive ? styles.tabActive : {}),
      }}
      onClick={onSelect}
    >
      <span className="ai-subprocess-tab-icon" style={styles.tabIcon}>{icon}</span>
      {isRunning && <span className="ai-subprocess-pulse" style={styles.tabPulse} />}
      {isPending && <span className="ai-subprocess-pending-dot" style={styles.tabPendingDot} />}
      <span className="ai-subprocess-agent-badge" style={styles.agentBadge}>{agentLabel}</span>
      <span className="ai-subprocess-tab-label" style={styles.tabLabel}>{label}</span>
      <StatusBadge status={subProcess.status} />
      <button
        className="ai-subprocess-tab-close"
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
      className={`ai-subprocess-status ai-subprocess-status-${status}`}
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

const statusConfig: Record<SubProcess["status"], { label: string; color: string; bg: string }> = {
  pending_approval: {
    label: "待审查",
    color: "#f59e0b",
    bg: "rgba(245,158,11,0.1)",
  },
  running: { label: "运行中", color: "#22c55e", bg: "rgba(34,197,94,0.1)" },
  stopped: { label: "已停止", color: "#0f766e", bg: "rgba(15,118,110,0.12)" },
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
  agentBadge: {
    padding: "1px 6px",
    borderRadius: "999px",
    background: "var(--bg-card)",
    border: "1px solid var(--border-primary)",
    color: "var(--text-tertiary)",
    fontSize: "10px",
    fontWeight: 700,
    letterSpacing: "0.02em",
  },
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
  terminalStage: {
    width: "100%",
    height: "100%",
    position: "relative" as const,
  },
  terminalLayer: {
    position: "absolute" as const,
    inset: 0,
  },
  terminalWrap: {
    width: "100%",
    height: "100%",
    position: "relative" as const,
  },
  terminalStatusOverlay: {
    position: "absolute" as const,
    top: 8,
    right: 8,
    padding: "6px 10px",
    borderRadius: "8px",
    fontSize: "12px",
    lineHeight: 1.45,
    color: "var(--text-primary)",
    background: "color-mix(in srgb, var(--bg-elevated) 92%, transparent)",
    border: "1px solid var(--border-primary)",
    boxShadow: "0 8px 20px rgba(0,0,0,0.12)",
    maxWidth: "min(520px, calc(100% - 16px))",
    pointerEvents: "none" as const,
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
