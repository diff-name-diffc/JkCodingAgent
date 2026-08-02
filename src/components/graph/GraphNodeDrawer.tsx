import { useEffect, useMemo, useState } from "react";
import { motion } from "framer-motion";
import { invoke } from "@tauri-apps/api/core";
import {
  ArrowDown,
  ArrowUp,
  Bot,
  ChevronDown,
  ChevronRight,
  FileCode2,
  RotateCcw,
  Sparkles,
  Square,
  Terminal,
  X,
} from "lucide-react";
import type {
  GraphDefinition,
  GraphPlanStatus,
  SubAgentEvent,
  SubAgentRunTrace,
} from "../../types";
import { cn } from "../../lib/cn";
import { useToast } from "../Toast";
import { Button } from "../ui/button";
import { Textarea } from "../ui/textarea";
import { SubAgentExecutionCard } from "../SubAgentExecutionView";
import {
  getSubAgentSession,
  hydrateSubAgentTrace,
  useSubAgentSessions,
} from "../subAgentEventStore";
import { hydrateGraphPlan, useGraphPlan } from "./graph-store";
import {
  NODE_STATUS_META,
  formatGraphDuration,
  normalizeNodeStatus,
  parseGraphDefinition,
} from "./graph-utils";

export interface GraphNodeDrawerProps {
  planId: string;
  nodeId: string;
  /** 整图状态：决定底部操作区展示「停止整图」还是「重新执行整图」。 */
  planStatus: GraphPlanStatus;
  actionPending: boolean;
  onClose: () => void;
  /** 点击上/下游依赖 chip 时跳转到对应节点。 */
  onSelectNode: (nodeId: string) => void;
  onStart: () => void;
  onCancel: () => void;
}

/**
 * 节点详情抽屉：任务指令（draft 可编辑）/ 上下游依赖 / 影响文件 /
 * 实时与最终输出 / 错误 / 整图级重试与停止。
 * subAgent 节点的执行轨迹直接复用 subAgentEventStore + SubAgentExecutionCard
 * （traceToolCallId = `graphnode:{planId}:{nodeId}`，与 sub-agent-event 载荷对齐）。
 */
export function GraphNodeDrawer({
  planId,
  nodeId,
  planStatus,
  actionPending,
  onClose,
  onSelectNode,
  onStart,
  onCancel,
}: GraphNodeDrawerProps) {
  const { showToast } = useToast();
  const snapshot = useGraphPlan(planId);
  const plan = snapshot.plan;
  const definition = useMemo(() => parseGraphDefinition(plan), [plan]);
  const nodeDef = definition?.nodes.find((node) => node.id === nodeId) ?? null;
  const nodeRun = plan?.nodeRuns.find((run) => run.nodeId === nodeId) ?? null;
  const workspaceId = plan?.workspaceId ?? "";

  const status = normalizeNodeStatus(nodeRun?.status ?? "pending");
  const statusMeta = NODE_STATUS_META[status];
  const duration = formatGraphDuration(nodeRun?.durationMs);
  const isSubAgent = nodeDef?.agent.kind === "subAgent";
  const agentLabel = !nodeDef
    ? ""
    : nodeDef.agent.kind === "subAgent"
      ? `子智能体 · ${nodeDef.agent.agentId}`
      : nodeDef.agent.kind === "claude"
        ? "Claude"
        : "Codex";
  const AgentIcon = !nodeDef ? Bot : nodeDef.agent.kind === "subAgent" ? Bot : nodeDef.agent.kind === "claude" ? Terminal : Sparkles;

  // ── 任务指令编辑（仅 draft 态；失焦整体提交 definition，失败回滚） ──
  const editable = plan?.status === "draft" && nodeDef !== null;
  const [draftTask, setDraftTask] = useState(nodeDef?.task ?? "");
  const [savingTask, setSavingTask] = useState(false);
  useEffect(() => {
    setDraftTask(nodeDef?.task ?? "");
  }, [nodeDef?.task, nodeId]);

  const handleTaskBlur = async () => {
    if (!editable || !plan || !definition || !nodeDef) return;
    const nextTask = draftTask.trim();
    if (nextTask === nodeDef.task) return;
    if (!nextTask) {
      setDraftTask(nodeDef.task);
      return;
    }
    setSavingTask(true);
    try {
      const nextDefinition: GraphDefinition = {
        ...definition,
        nodes: definition.nodes.map((node) =>
          node.id === nodeId ? { ...node, task: nextTask } : node,
        ),
      };
      await invoke("graph_plan_update", {
        planId,
        definitionJson: JSON.stringify(nextDefinition),
      });
      await hydrateGraphPlan(planId);
      showToast("节点指令已保存");
    } catch (err) {
      // 后端校验失败返回中文错误字符串，原样透出并回滚编辑。
      showToast(`保存节点指令失败：${err instanceof Error ? err.message : String(err)}`, "warning");
      setDraftTask(nodeDef.task);
    } finally {
      setSavingTask(false);
    }
  };

  // ── subAgent 节点轨迹：优先 live store，缺失时从持久化 trace 回放 ──
  const traceToolCallId = isSubAgent ? (nodeRun?.traceToolCallId ?? null) : null;
  const subAgentSessions = useSubAgentSessions(workspaceId);
  const subAgentSession = traceToolCallId
    ? (subAgentSessions[traceToolCallId] ?? null)
    : null;
  const [traceLoading, setTraceLoading] = useState(false);
  const [traceError, setTraceError] = useState<string | null>(null);

  useEffect(() => {
    setTraceError(null);
    if (!traceToolCallId || !workspaceId) return;
    if (getSubAgentSession(workspaceId, traceToolCallId)) return;
    // 运行中/未开始：等 sub-agent-event 实时流入，不做历史回放。
    if (status === "running" || status === "pending") return;
    let cancelled = false;
    setTraceLoading(true);
    invoke<SubAgentRunTrace | null>("sub_agent_get_run_trace", {
      workspaceId,
      toolCallId: traceToolCallId,
    })
      .then((trace) => {
        if (cancelled || !trace) return;
        const parsed: unknown = JSON.parse(trace.eventsJson);
        if (Array.isArray(parsed)) {
          hydrateSubAgentTrace(workspaceId, traceToolCallId, parsed as SubAgentEvent[]);
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setTraceError(err instanceof Error ? err.message : String(err));
        }
      })
      .finally(() => {
        if (!cancelled) setTraceLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [workspaceId, traceToolCallId, status]);

  const [inputExpanded, setInputExpanded] = useState(false);
  const liveOutput = snapshot.liveOutputs[nodeId] ?? "";
  const finalOutput = nodeRun?.outputText ?? "";
  const cliOutput = liveOutput || finalOutput;

  // ── 上下游依赖（chip 可点击跳转；带状态点） ──
  const statusOf = (id: string) =>
    normalizeNodeStatus(plan?.nodeRuns.find((run) => run.nodeId === id)?.status ?? "pending");
  const upstreamIds = useMemo(
    () => (nodeDef?.dependsOn ?? []).filter((id) => definition?.nodes.some((n) => n.id === id)),
    [nodeDef, definition],
  );
  const downstreamIds = useMemo(
    () => (definition?.nodes ?? []).filter((n) => n.dependsOn.includes(nodeId)).map((n) => n.id),
    [definition, nodeId],
  );
  const titleOf = (id: string) =>
    definition?.nodes.find((n) => n.id === id)?.title ?? id;

  const affectedFiles = nodeRun?.affectedFiles ?? [];
  const showStop = planStatus === "running";
  const showRestart =
    planStatus === "failed" || planStatus === "cancelled" || planStatus === "completed";

  return (
    <motion.aside
      initial={{ x: 420, opacity: 0 }}
      animate={{ x: 0, opacity: 1 }}
      exit={{ x: 420, opacity: 0 }}
      transition={{ duration: 0.2, ease: [0.2, 0.8, 0.2, 1] }}
      className="ai-graph-drawer"
    >
      <div className="ai-graph-drawer-header">
        <span className={cn("ai-graph-drawer-agent", `ai-graph-drawer-agent--${nodeDef?.agent.kind ?? "subAgent"}`)}>
          <AgentIcon className="h-3.5 w-3.5" aria-hidden />
          {agentLabel}
        </span>
        <span className={cn("ai-graph-chip", `ai-graph-chip--node-${status}`)}>
          {statusMeta.label}
        </span>
        {duration && <span className="ai-graph-drawer-duration">{duration}</span>}
        <Button variant="ghost" size="icon-sm" aria-label="关闭节点详情" onClick={onClose}>
          <X className="h-4 w-4" />
        </Button>
      </div>

      <div className="ai-graph-drawer-body">
        <h3 className="ai-graph-drawer-title">{nodeDef?.title ?? nodeId}</h3>

        {nodeDef?.role && (
          <section className="ai-graph-drawer-section">
            <div className="ai-graph-drawer-label">角色</div>
            <p className="ai-graph-drawer-text">{nodeDef.role}</p>
          </section>
        )}

        <section className="ai-graph-drawer-section">
          <div className="ai-graph-drawer-label">
            任务指令
            {savingTask && <span className="ai-graph-drawer-hint">保存中…</span>}
            {editable && !savingTask && <span className="ai-graph-drawer-hint">可编辑，失焦保存</span>}
          </div>
          {editable ? (
            <Textarea
              value={draftTask}
              onChange={(event) => setDraftTask(event.target.value)}
              onBlur={() => void handleTaskBlur()}
              rows={8}
              aria-label="节点任务指令"
              className="resize-y font-mono text-xs leading-relaxed"
            />
          ) : (
            <pre className="ai-graph-drawer-pre">{nodeDef?.task ?? ""}</pre>
          )}
        </section>

        {nodeRun?.inputText && (
          <section className="ai-graph-drawer-section">
            <button
              type="button"
              className="ai-graph-drawer-label ai-graph-drawer-label--toggle"
              onClick={() => setInputExpanded((value) => !value)}
              aria-expanded={inputExpanded}
            >
              {inputExpanded ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
              上游输入（装配后）
            </button>
            {inputExpanded && <pre className="ai-graph-drawer-pre">{nodeRun.inputText}</pre>}
          </section>
        )}

        {(upstreamIds.length > 0 || downstreamIds.length > 0) && (
          <section className="ai-graph-drawer-section">
            <div className="ai-graph-drawer-label">依赖关系</div>
            {upstreamIds.length > 0 && (
              <div className="ai-graph-drawer-deps-row">
                <span className="ai-graph-drawer-deps-kind">
                  <ArrowUp className="h-3 w-3" aria-hidden />
                  上游
                </span>
                <div className="ai-graph-drawer-deps-chips">
                  {upstreamIds.map((id) => (
                    <button
                      key={id}
                      type="button"
                      className="ai-graph-dep-chip"
                      title={titleOf(id)}
                      onClick={() => onSelectNode(id)}
                    >
                      <span
                        className={cn("ai-graph-dep-dot", `ai-graph-dep-dot--${statusOf(id)}`)}
                        aria-hidden
                      />
                      {titleOf(id)}
                    </button>
                  ))}
                </div>
              </div>
            )}
            {downstreamIds.length > 0 && (
              <div className="ai-graph-drawer-deps-row">
                <span className="ai-graph-drawer-deps-kind">
                  <ArrowDown className="h-3 w-3" aria-hidden />
                  下游
                </span>
                <div className="ai-graph-drawer-deps-chips">
                  {downstreamIds.map((id) => (
                    <button
                      key={id}
                      type="button"
                      className="ai-graph-dep-chip"
                      title={titleOf(id)}
                      onClick={() => onSelectNode(id)}
                    >
                      <span
                        className={cn("ai-graph-dep-dot", `ai-graph-dep-dot--${statusOf(id)}`)}
                        aria-hidden
                      />
                      {titleOf(id)}
                    </button>
                  ))}
                </div>
              </div>
            )}
          </section>
        )}

        {isSubAgent ? (
          <section className="ai-graph-drawer-section">
            <div className="ai-graph-drawer-label">执行轨迹</div>
            {subAgentSession ? (
              <SubAgentExecutionCard session={subAgentSession} autoExpand />
            ) : traceLoading ? (
              <p className="ai-graph-drawer-hint">轨迹加载中…</p>
            ) : traceError ? (
              <p className="ai-graph-drawer-error">{traceError}</p>
            ) : status === "running" || status === "pending" ? (
              <p className="ai-graph-drawer-hint">等待子智能体事件…</p>
            ) : (
              <p className="ai-graph-drawer-hint">未记录执行轨迹。</p>
            )}
          </section>
        ) : (
          cliOutput && (
            <section className="ai-graph-drawer-section">
              <div className="ai-graph-drawer-label">
                {liveOutput ? "实时输出" : "输出"}
              </div>
              <pre className="ai-graph-drawer-pre ai-graph-drawer-pre--output">{cliOutput}</pre>
            </section>
          )
        )}

        {affectedFiles.length > 0 && (
          <section className="ai-graph-drawer-section">
            <div className="ai-graph-drawer-label">影响文件（{affectedFiles.length}）</div>
            <ul className="ai-graph-drawer-files">
              {affectedFiles.map((file) => (
                <li key={file} className="ai-graph-drawer-file" title={file}>
                  <FileCode2 className="h-3 w-3" aria-hidden />
                  <span className="ai-graph-drawer-file-path">{file}</span>
                </li>
              ))}
            </ul>
          </section>
        )}

        {nodeRun?.errorText && (
          <section className="ai-graph-drawer-section">
            <div className="ai-graph-drawer-label">错误</div>
            <div className="ai-graph-drawer-error">{nodeRun.errorText}</div>
          </section>
        )}
      </div>

      {(showStop || showRestart) && (
        <div className="ai-graph-drawer-footer">
          {showStop && (
            <Button
              size="sm"
              variant="destructive"
              onClick={onCancel}
              disabled={actionPending}
            >
              <Square className="h-3.5 w-3.5" />
              停止整图执行
            </Button>
          )}
          {showRestart && (
            <Button
              size="sm"
              variant="outline"
              onClick={onStart}
              disabled={actionPending}
            >
              <RotateCcw className="h-3.5 w-3.5" />
              重新执行整图
            </Button>
          )}
        </div>
      )}
    </motion.aside>
  );
}
