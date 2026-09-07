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
  | "verdict"
  | "connection"
  | "mcp-server";

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

/**
 * MCP 连接聚合状态（UI-22b）：头部指示灯与弹窗健康度共用。
 * 不再把 degraded 与 invalid_config 压成单一「异常」——两者 tone 同为 error
 * 但 label 区分原因，异常详情（真实失败服务器/原因）在弹窗内按服务器展开。
 * `checking` 为前端取数中态（非后端 aggregate 值）。
 */
const CONNECTION_STATUS: Record<string, StatusMeta> = {
  checking: { tone: "running", label: "检查中" },
  not_configured: { tone: "neutral", label: "未配置" },
  healthy: { tone: "success", label: "正常" },
  degraded: { tone: "error", label: "异常" },
  invalid_config: { tone: "error", label: "配置无效" },
};

/**
 * MCP 单服务器状态（UI-22b）：弹窗内服务器卡双编码（图标 + 文字），
 * 取代旧「纯色点 + meta 行文字」。tone 对齐旧 stateColor 语义
 * （spawn_failed=warn 可重试，invalid_config/connection_failed=error）。
 */
const MCP_SERVER_STATUS: Record<string, StatusMeta> = {
  disabled: { tone: "neutral", label: "已禁用" },
  healthy: { tone: "success", label: "正常" },
  invalid_config: { tone: "error", label: "配置无效" },
  spawn_failed: { tone: "warn", label: "启动失败" },
  connection_failed: { tone: "error", label: "连接失败" },
};

const DOMAIN_STATUS: Record<StatusDomain, Record<string, StatusMeta>> = {
  tool: TOOL_STATUS,
  "graph-node": GRAPH_NODE_STATUS,
  "graph-plan": GRAPH_PLAN_STATUS,
  python: PYTHON_STATUS,
  subagent: SUBAGENT_STATUS,
  verdict: VERDICT_STATUS,
  connection: CONNECTION_STATUS,
  "mcp-server": MCP_SERVER_STATUS,
};

export function resolveStatusMeta(domain: StatusDomain, status: string): StatusMeta {
  return DOMAIN_STATUS[domain][status] ?? { tone: "neutral", label: status };
}
