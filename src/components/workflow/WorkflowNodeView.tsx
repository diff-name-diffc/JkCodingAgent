import { memo } from "react";
import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";
import { Wrench } from "lucide-react";
import type { WorkflowKnownNodePhase, WorkflowNodePhase, WorkflowNodeStatus } from "../../types";
import { cn } from "../../lib/cn";
import { StatusPill } from "../detail/StatusPill";
import { NODE_STATUS_META, formatWorkflowDuration } from "./workflow-utils";

/** React Flow 自定义节点携带的数据（由 WorkflowPanel 组装）。 */
export interface WorkflowFlowNodeData extends Record<string, unknown> {
  nodeId: string;
  title: string;
  modelLabel: string;
  task: string;
  outputPreview: string;
  status: WorkflowNodeStatus;
  phase: WorkflowNodePhase;
  durationMs: number | null;
  toolCallCount: number;
  streaming: boolean;
}

export type WorkflowFlowNode = Node<WorkflowFlowNodeData, "workflowNode">;

const PHASE_LABEL: Record<WorkflowKnownNodePhase, string> = {
  starting: "准备上下文",
  thinking: "分析中",
  responding: "生成响应",
  tool_running: "调用工具",
  cached: "复用结果",
  finalizing: "收尾",
};

function phaseLabel(phase: WorkflowNodePhase): string {
  return PHASE_LABEL[phase as WorkflowKnownNodePhase] ?? phase;
}

/**
 * 工作流编排画布的自定义节点（UI-13 压缩版）。
 * 信息层级：任务名（一级，单行省略）→ 角色/模型（二级小字）→ 结果摘要
 * （最多两行，实时输出优先）→ 状态 pill + 工具数 + 耗时。
 * 完整任务描述与输出在节点抽屉（WorkflowNodeDrawer），卡面不再平铺。
 * 连接锚点默认隐藏，仅在悬浮/选中时浮现（见 .ai-workflow-node-handle 样式）。
 */
export const WorkflowNodeView = memo(function WorkflowNodeView({
  data,
  selected,
}: NodeProps<WorkflowFlowNode>) {
  const duration = formatWorkflowDuration(data.durationMs);
  const summary = data.outputPreview || data.task;
  const statusMeta = NODE_STATUS_META[data.status] ?? NODE_STATUS_META.pending;
  const streamingLabel = `${statusMeta.label} · ${phaseLabel(data.phase)}`;

  return (
    <div
      className={cn(
        "ai-workflow-node",
        `ai-workflow-node--${data.status}`,
        selected && "ai-workflow-node--selected",
      )}
    >
      <Handle type="target" position={Position.Top} className="ai-workflow-node-handle" />
      <span className="ai-workflow-node-status-ring" aria-hidden />
      <div className="ai-workflow-node-main">
        <div className="ai-workflow-node-title" title={data.title}>
          {data.title}
        </div>
        <div className="ai-workflow-node-agent" title={data.modelLabel || "Claude Agent"}>
          {data.modelLabel || "Claude Agent"}
        </div>
        {summary && (
          <div className="ai-workflow-node-summary" title={summary}>
            {summary}
          </div>
        )}
        <div className="ai-workflow-node-meta-row">
          <StatusPill
            domain="workflow-node"
            status={data.status}
            label={data.streaming ? streamingLabel : undefined}
          />
          {data.toolCallCount > 0 && (
            <span className="ai-workflow-node-duration">
              <Wrench className="h-3 w-3" />
              {data.toolCallCount}
            </span>
          )}
          {duration && <span className="ai-workflow-node-duration">{duration}</span>}
        </div>
      </div>
      <Handle type="source" position={Position.Bottom} className="ai-workflow-node-handle" />
    </div>
  );
});
