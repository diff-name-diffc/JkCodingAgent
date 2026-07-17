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
        display: isPanelVisible ? "flex" : "none",
        height: isExpanded ? height : COLLAPSED_HEIGHT,
      }}
    >
      {/* Resize handle */}
      {isExpanded && <div className="ai-subprocess-resize" onMouseDown={onResizeStart} />}

      {/* Tab bar */}
      <div className="ai-subprocess-tabbar">
        <div className="ai-subprocess-tabbar-label">子进程终端</div>
        <div className="ai-subprocess-tablist">
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
        <div className="ai-subprocess-content" style={{ display: activeSubProcess ? "flex" : "none" }}>
          {activeSubProcess?.status === "pending_approval" ? (
            <div className="ai-subprocess-placeholder">
              <span>⏳ 等待审批...</span>
            </div>
          ) : (
            <div className="ai-subprocess-terminal-stage">
              {mountedTerminals.map((subProcess) => {
                const taskId = subProcess.id;
                const isVisible = subProcess.id === activeTabId;
                const isInteractive = subProcess.status === "running";
                return (
                  <div
                    key={subProcess.id}
                    className="ai-subprocess-terminal-layer"
                    style={{ display: isVisible ? "block" : "none" }}
                  >
                    <div className="ai-subprocess-terminal-wrap">
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
                        isActive={isVisible && isInteractive}
                        {...getRestoreState(taskId)}
                      />
                      {!isInteractive && isVisible && (
                        <div className="ai-subprocess-status-overlay">
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
      onClick={onSelect}
    >
      <span className="ai-subprocess-tab-icon">{icon}</span>
      {isRunning && <span className="ai-subprocess-pulse" />}
      {isPending && <span className="ai-subprocess-pending-dot" />}
      <span className="ai-subprocess-agent-badge">{agentLabel}</span>
      <span className="ai-subprocess-tab-label">{label}</span>
      <StatusBadge status={subProcess.status} />
      <button
        className="ai-subprocess-tab-close"
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
      style={{ color: config.color, background: config.bg }}
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
