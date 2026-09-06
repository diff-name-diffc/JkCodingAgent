/**
 * 状态双编码映射（UI-12 建立，UI-13/14/18/20 共用）。
 *
 * tone 驱动 StatusPill 的配色与图标形状，label 复用各领域的既有中文文案
 * （graph 两域直接引用 graph-utils 的 PLAN/NODE_STATUS_META，保持单一出处）。
 * 未知状态一律回退 neutral + 原文——绝不虚构成功（tokens.md §5 评审结论 4：
 * 状态色一律双编码「图标 + 文字」，不得只靠彩点）。
 */
import { NODE_STATUS_META, PLAN_STATUS_META } from "../graph/graph-utils";

export type StatusTone = "success" | "error" | "running" | "warn" | "neutral" | "pending";

export type StatusDomain =
  | "tool"
  | "graph-node"
  | "graph-plan"
  | "python"
  | "subagent"
  | "verdict";

export interface StatusMeta {
  tone: StatusTone;
  label: string;
}

/** 工具卡状态（ToolCallStatus + 前端 planned 标记；planned 独立于 running 表达「等待」）。 */
const TOOL_STATUS: Record<string, StatusMeta> = {
  planned: { tone: "pending", label: "等待" },
  running: { tone: "running", label: "执行中" },
  success: { tone: "success", label: "成功" },
  error: { tone: "error", label: "失败" },
};

const GRAPH_NODE_STATUS: Record<string, StatusMeta> = {
  pending: { tone: "pending", label: NODE_STATUS_META.pending.label },
  running: { tone: "running", label: NODE_STATUS_META.running.label },
  succeeded: { tone: "success", label: NODE_STATUS_META.succeeded.label },
  failed: { tone: "error", label: NODE_STATUS_META.failed.label },
  skipped: { tone: "neutral", label: NODE_STATUS_META.skipped.label },
  cancelled: { tone: "warn", label: NODE_STATUS_META.cancelled.label },
};

const GRAPH_PLAN_STATUS: Record<string, StatusMeta> = {
  draft: { tone: "pending", label: PLAN_STATUS_META.draft.label },
  running: { tone: "running", label: PLAN_STATUS_META.running.label },
  completed: { tone: "success", label: PLAN_STATUS_META.completed.label },
  failed: { tone: "error", label: PLAN_STATUS_META.failed.label },
  cancelled: { tone: "warn", label: PLAN_STATUS_META.cancelled.label },
};

/** PythonCodeRunStatus + 抽屉空态（idle=未选中/未运行记录）。 */
const PYTHON_STATUS: Record<string, StatusMeta> = {
  running: { tone: "running", label: "运行中" },
  done: { tone: "success", label: "已完成" },
  failed: { tone: "error", label: "执行失败" },
  stopped: { tone: "warn", label: "已停止" },
  idle: { tone: "neutral", label: "未运行" },
};

const SUBAGENT_STATUS: Record<string, StatusMeta> = {
  running: { tone: "running", label: "运行中" },
  completed: { tone: "success", label: "已完成" },
  failed: { tone: "error", label: "失败" },
};

/** GraphRunSummary.verdictStatus：运行结果之外的独立验收结论。
 * unknown 默认「未能验收」（终态但验证器未给出结论）；运行中尚无验收
 * 对象的场景由调用方以 label 覆盖为「验收未开始」（GraphPanelHeader）。 */
const VERDICT_STATUS: Record<string, StatusMeta> = {
  pass: { tone: "success", label: "验收通过" },
  partial: { tone: "warn", label: "部分通过" },
  fail: { tone: "error", label: "验收失败" },
  unknown: { tone: "neutral", label: "未能验收" },
};

const DOMAIN_STATUS: Record<StatusDomain, Record<string, StatusMeta>> = {
  tool: TOOL_STATUS,
  "graph-node": GRAPH_NODE_STATUS,
  "graph-plan": GRAPH_PLAN_STATUS,
  python: PYTHON_STATUS,
  subagent: SUBAGENT_STATUS,
  verdict: VERDICT_STATUS,
};

export function resolveStatusMeta(domain: StatusDomain, status: string): StatusMeta {
  return DOMAIN_STATUS[domain][status] ?? { tone: "neutral", label: status };
}
