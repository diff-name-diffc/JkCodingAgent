import dagre from "dagre";
import type { WorkflowDefinition } from "../../types";

/**
 * dagre 分层布局（rankdir=TB，统一自上而下）：输入工作流定义，输出各节点在
 * React Flow 画布上的左上角坐标。节点尺寸与 WorkflowNodeView 的 CSS 保持一致。
 */

export const WORKFLOW_NODE_WIDTH = 264;
/** UI-13 节点卡压缩后：标题单行 + 模型小字 + 摘要两行 + 状态行。 */
export const WORKFLOW_NODE_HEIGHT = 104;
/**
 * 布局估算高度：与压缩后节点卡的实际渲染高度对齐
 * （22 padding + 17 标题 + 13 模型 + 29 摘要两行 + 18 状态行 + 15 间距）。
 */
const WORKFLOW_NODE_LAYOUT_HEIGHT = 116;

export interface WorkflowNodePosition {
  x: number;
  y: number;
}

export function computeWorkflowLayout(
  definition: WorkflowDefinition,
): Map<string, WorkflowNodePosition> {
  const layoutGraph = new dagre.graphlib.Graph();
  layoutGraph.setGraph({
    rankdir: "TB",
    // 节点间距适当放宽，避免默认排版过于拥挤
    nodesep: 88,
    ranksep: 128,
    marginx: 40,
    marginy: 40,
  });
  layoutGraph.setDefaultEdgeLabel(() => ({}));

  for (const node of definition.nodes) {
    layoutGraph.setNode(node.id, {
      width: WORKFLOW_NODE_WIDTH,
      height: WORKFLOW_NODE_LAYOUT_HEIGHT,
    });
  }
  const nodeIds = new Set(definition.nodes.map((node) => node.id));
  for (const node of definition.nodes) {
    for (const dependency of node.dependsOn) {
      // 依赖缺失属于校验失败场景，布局阶段直接跳过，避免 dagre 抛错。
      if (nodeIds.has(dependency)) {
        layoutGraph.setEdge(dependency, node.id);
      }
    }
  }

  dagre.layout(layoutGraph);

  const positions = new Map<string, WorkflowNodePosition>();
  for (const node of definition.nodes) {
    const laidOut = layoutGraph.node(node.id);
    if (!laidOut) continue;
    // dagre 输出中心点坐标，React Flow 需要左上角；
    // 取整到整数像素，避免半像素平移导致文本发虚。
    positions.set(node.id, {
      x: Math.round(laidOut.x - WORKFLOW_NODE_WIDTH / 2),
      y: Math.round(laidOut.y - WORKFLOW_NODE_LAYOUT_HEIGHT / 2),
    });
  }
  return positions;
}
