import dagre from "dagre";
import type { GraphDefinition } from "../../types";

/**
 * dagre 分层布局（rankdir=TB，统一自上而下）：输入图定义，输出各节点在
 * React Flow 画布上的左上角坐标。节点尺寸与 GraphNodeView 的 CSS 保持一致。
 */

export const GRAPH_NODE_WIDTH = 264;
export const GRAPH_NODE_HEIGHT = 88;

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
    nodesep: 44,
    ranksep: 72,
    marginx: 32,
    marginy: 32,
  });
  graph.setDefaultEdgeLabel(() => ({}));

  for (const node of definition.nodes) {
    graph.setNode(node.id, { width: GRAPH_NODE_WIDTH, height: GRAPH_NODE_HEIGHT });
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
    // dagre 输出中心点坐标，React Flow 需要左上角。
    positions.set(node.id, {
      x: laidOut.x - GRAPH_NODE_WIDTH / 2,
      y: laidOut.y - GRAPH_NODE_HEIGHT / 2,
    });
  }
  return positions;
}
