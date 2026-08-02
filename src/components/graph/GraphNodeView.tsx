import { memo } from "react";
import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";
import { Bot, Sparkles, Terminal } from "lucide-react";
import type { GraphNodeStatus } from "../../types";
import { cn } from "../../lib/cn";
import { NODE_STATUS_META, formatGraphDuration } from "./graph-utils";

/** React Flow 自定义节点携带的数据（由 GraphPanel 组装）。 */
export interface GraphFlowNodeData extends Record<string, unknown> {
  nodeId: string;
  title: string;
  agentKind: string;
  agentId: string | null;
  status: GraphNodeStatus;
  durationMs: number | null;
  /** CLI 节点运行中（行流式输出中）。 */
  streaming: boolean;
}

export type GraphFlowNode = Node<GraphFlowNodeData, "graphNode">;

function AgentKindIcon({ agentKind }: { agentKind: string }) {
  const className = "ai-graph-node-agent-icon";
  if (agentKind === "subAgent") return <Bot className={className} aria-hidden />;
  if (agentKind === "claude") return <Terminal className={className} aria-hidden />;
  return <Sparkles className={className} aria-hidden />;
}

function agentKindLabel(agentKind: string, agentId: string | null): string {
  if (agentKind === "subAgent") return agentId ? `子智能体 · ${agentId}` : "子智能体";
  if (agentKind === "claude") return "Claude";
  if (agentKind === "codex") return "Codex";
  return agentKind;
}

/**
 * 图编排画布的自定义节点。
 * 信息层级：任务标题（两行省略，悬浮 title 显示全文）→ 执行 Agent → 状态 + 耗时；
 * 连接锚点默认隐藏，仅在悬浮/选中时浮现（见 .ai-graph-node-handle 样式）。
 */
export const GraphNodeView = memo(function GraphNodeView({
  data,
  selected,
}: NodeProps<GraphFlowNode>) {
  const statusMeta = NODE_STATUS_META[data.status] ?? NODE_STATUS_META.pending;
  const duration = formatGraphDuration(data.durationMs);

  return (
    <div
      className={cn(
        "ai-graph-node",
        `ai-graph-node--${data.status}`,
        selected && "ai-graph-node--selected",
      )}
    >
      <Handle type="target" position={Position.Top} className="ai-graph-node-handle" />
      <span className="ai-graph-node-status-ring" aria-hidden />
      <div className="ai-graph-node-main">
        <div className="ai-graph-node-title" title={data.title}>
          {data.title}
        </div>
        <div className="ai-graph-node-agent-row">
          <AgentKindIcon agentKind={data.agentKind} />
          <span className="ai-graph-node-agent">
            {agentKindLabel(data.agentKind, data.agentId)}
          </span>
        </div>
        <div className="ai-graph-node-meta-row">
          <span className={cn("ai-graph-node-status", `ai-graph-node-status--${data.status}`)}>
            {statusMeta.label}
            {data.streaming && "…"}
          </span>
          {duration && <span className="ai-graph-node-duration">{duration}</span>}
        </div>
      </div>
      <Handle type="source" position={Position.Bottom} className="ai-graph-node-handle" />
    </div>
  );
});
