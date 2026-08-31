import { memo } from "react";
import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";
import { Activity, BrainCircuit, Wrench } from "lucide-react";
import type { GraphKnownNodePhase, GraphNodePhase, GraphNodeStatus } from "../../types";
import { cn } from "../../lib/cn";
import { NODE_STATUS_META, formatGraphDuration } from "./graph-utils";

/** React Flow 自定义节点携带的数据（由 GraphPanel 组装）。 */
export interface GraphFlowNodeData extends Record<string, unknown> {
  nodeId: string;
  title: string;
  modelLabel: string;
  task: string;
  outputPreview: string;
  status: GraphNodeStatus;
  phase: GraphNodePhase;
  durationMs: number | null;
  toolCallCount: number;
  streaming: boolean;
}

export type GraphFlowNode = Node<GraphFlowNodeData, "graphNode">;

const PHASE_LABEL: Record<GraphKnownNodePhase, string> = {
  starting: "准备上下文",
  thinking: "分析中",
  responding: "生成响应",
  tool_running: "调用工具",
  retrying: "重试中",
  compacting: "压缩上下文",
  cached: "复用结果",
  finalizing: "收尾",
};

function phaseLabel(phase: GraphNodePhase): string {
  return PHASE_LABEL[phase as GraphKnownNodePhase] ?? phase;
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
          <BrainCircuit className="ai-graph-node-agent-icon" aria-hidden />
          <span className="ai-graph-node-agent">{data.modelLabel || "PI Agent"}</span>
        </div>
        <div className="ai-graph-node-task" title={data.task}>
          {data.task}
        </div>
        {data.outputPreview && (
          <div className="ai-graph-node-output" title={data.outputPreview}>
            {data.outputPreview}
          </div>
        )}
        <div className="ai-graph-node-meta-row">
          <span className={cn("ai-graph-node-status", `ai-graph-node-status--${data.status}`)}>
            {statusMeta.label}
            {data.streaming && ` · ${phaseLabel(data.phase)}`}
          </span>
          {data.streaming && <Activity className="h-3 w-3" aria-hidden />}
          {data.toolCallCount > 0 && (
            <span className="ai-graph-node-duration">
              <Wrench className="h-3 w-3" />
              {data.toolCallCount}
            </span>
          )}
          {duration && <span className="ai-graph-node-duration">{duration}</span>}
        </div>
      </div>
      <Handle type="source" position={Position.Bottom} className="ai-graph-node-handle" />
    </div>
  );
});
