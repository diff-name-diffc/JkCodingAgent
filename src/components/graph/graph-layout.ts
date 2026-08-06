import dagre from "dagre";
import type { GraphDefinition } from "../../types";

/**
 * dagre 分层布局（rankdir=TB，统一自上而下）：输入图定义，输出各节点在
 * React Flow 画布上的左上角坐标。节点尺寸与 GraphNodeView 的 CSS 保持一致。
 */

export const GRAPH_NODE_WIDTH = 264;
export const GRAPH_NODE_HEIGHT = 88;
/**
 * 布局估算高度：节点带任务摘要/输出预览行时实际渲染更高，
 * 用它参与 dagre 排版避免层与层之间视觉重叠。
 */
const GRAPH_NODE_LAYOUT_HEIGHT = 136;

export interface GraphNodePosition {
  x: number;
  y: number;
}

export function computeGraphLayout(
  definition: GraphDefinition,
): Map<string, GraphNodePosition> {
  const graph = new dagre.graphlib.Graph();
  graph.setGraph({
    rankdir: "TB",
    // 节点间距适当放宽，避免默认排版过于拥挤
    nodesep: 88,
    ranksep: 128,
    marginx: 40,
    marginy: 40,
  });
  graph.setDefaultEdgeLabel(() => ({}));

  for (const node of definition.nodes) {
    graph.setNode(node.id, {
      width: GRAPH_NODE_WIDTH,
      height: GRAPH_NODE_LAYOUT_HEIGHT,
    });
  }
  const nodeIds = new Set(definition.nodes.map((node) => node.id));
  for (const node of definition.nodes) {
    for (const dependency of node.dependsOn) {
      // 依赖缺失属于校验失败场景，布局阶段直接跳过，避免 dagre 抛错。
      if (nodeIds.has(dependency)) {
        graph.setEdge(dependency, node.id);
      }
    }
  }

  dagre.layout(graph);

  const positions = new Map<string, GraphNodePosition>();
  for (const node of definition.nodes) {
    const laidOut = graph.node(node.id);
    if (!laidOut) continue;
    // dagre 输出中心点坐标，React Flow 需要左上角；
    // 取整到整数像素，避免半像素平移导致文本发虚。
    positions.set(node.id, {
      x: Math.round(laidOut.x - GRAPH_NODE_WIDTH / 2),
      y: Math.round(laidOut.y - GRAPH_NODE_LAYOUT_HEIGHT / 2),
    });
  }
  return positions;
}
