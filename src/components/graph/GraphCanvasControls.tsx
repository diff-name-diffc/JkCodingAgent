import { useCallback } from "react";
import { useReactFlow } from "@xyflow/react";
import { LayoutGrid, LocateFixed, Maximize, ZoomIn, ZoomOut } from "lucide-react";
import type { GraphNodeStatus } from "../../types";
import { cn } from "../../lib/cn";
import { GRAPH_NODE_HEIGHT, GRAPH_NODE_WIDTH } from "./graph-layout";

const FIT_VIEW_OPTIONS = { padding: 0.18, maxZoom: 1 } as const;

/** 画布左下角控制条：缩放 / 适应画布 / 定位当前任务 / 重置手动布局。 */
export function GraphCanvasControls({
  statusByNodeId,
  hasCustomLayout,
  onResetLayout,
}: {
  statusByNodeId: Map<string, GraphNodeStatus>;
  hasCustomLayout: boolean;
  onResetLayout: () => void;
}) {
  const { zoomIn, zoomOut, fitView, setCenter, getZoom, getNode } = useReactFlow();

  // 定位「当前」任务：优先运行中，其次失败；都没有则适应全图。
  const locateCurrent = useCallback(() => {
    const entries = [...statusByNodeId.entries()];
    const targetId =
      entries.find(([, status]) => status === "running")?.[0] ??
      entries.find(([, status]) => status === "failed")?.[0];
    const node = targetId ? getNode(targetId) : undefined;
    if (!node) {
      void fitView({ ...FIT_VIEW_OPTIONS, duration: 300 });
      return;
    }
    void setCenter(node.position.x + GRAPH_NODE_WIDTH / 2, node.position.y + GRAPH_NODE_HEIGHT / 2, {
      zoom: Math.max(getZoom(), 1),
      duration: 300,
    });
  }, [statusByNodeId, getNode, setCenter, getZoom, fitView]);

  return (
    <div className="ai-graph-controls">
      <button
        type="button"
        className="ai-graph-control-btn"
        aria-label="放大"
        onClick={() => void zoomIn({ duration: 200 })}
      >
        <ZoomIn className="h-4 w-4" />
      </button>
      <button
        type="button"
        className="ai-graph-control-btn"
        aria-label="缩小"
        onClick={() => void zoomOut({ duration: 200 })}
      >
        <ZoomOut className="h-4 w-4" />
      </button>
      <button
        type="button"
        className="ai-graph-control-btn"
        aria-label="适应画布"
        onClick={() => void fitView({ ...FIT_VIEW_OPTIONS, duration: 300 })}
      >
        <Maximize className="h-4 w-4" />
      </button>
      <button
        type="button"
        className="ai-graph-control-btn"
        aria-label="定位当前任务"
        onClick={locateCurrent}
      >
        <LocateFixed className="h-4 w-4" />
      </button>
      <button
        type="button"
        className={cn("ai-graph-control-btn", hasCustomLayout && "ai-graph-control-btn--active")}
        aria-label="重置节点布局"
        title="恢复自动排版"
        disabled={!hasCustomLayout}
        onClick={onResetLayout}
      >
        <LayoutGrid className="h-4 w-4" />
      </button>
    </div>
  );
}
