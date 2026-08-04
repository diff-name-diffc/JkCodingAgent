import { useCallback, useEffect, useMemo, useState } from "react";
import { AnimatePresence } from "framer-motion";
import {
  Background,
  BackgroundVariant,
  MarkerType,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  type Edge,
  type NodeTypes,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { invoke } from "@tauri-apps/api/core";
import { Play, RotateCcw, Square, X } from "lucide-react";
import type { GraphNodeStatus } from "../../types";
import { cn } from "../../lib/cn";
import { useToast } from "../Toast";
import { Button } from "../ui/button";
import { useGraphPlan } from "./graph-store";
import { computeGraphLayout } from "./graph-layout";
import { GraphNodeView, type GraphFlowNode } from "./GraphNodeView";
import { GraphNodeDrawer } from "./GraphNodeDrawer";
import { GraphCanvasControls } from "./GraphCanvasControls";
import { GraphStateInspector } from "./GraphStateInspector";
import {
  EDGE_STATE_COLOR,
  PLAN_STATUS_META,
  computeEdgeState,
  computeGraphLayers,
  normalizeNodeStatus,
  normalizePlanStatus,
  parseGraphDefinition,
} from "./graph-utils";

const nodeTypes: NodeTypes = { graphNode: GraphNodeView };

export interface GraphPanelProps {
  planId: string;
  onClose: () => void;
}

/**
 * 图编排全屏面板：React Flow 画布 + 两层头部（标题/状态/操作 + 任务统计/进度）
 * + 底部共享 state 检查器。
 * 操作语义：draft/confirmed →「确认执行」(graph_run_start)；failed/cancelled/
 * completed →「重新执行」（同一命令，后端创建新 attempt 并保留历史）；running →「停止」
 * (graph_run_cancel)；任何状态都可关闭面板（关闭不影响后台执行）。
 */
export function GraphPanel({ planId, onClose }: GraphPanelProps) {
  return (
    <ReactFlowProvider>
      <GraphPanelInner planId={planId} onClose={onClose} />
    </ReactFlowProvider>
  );
}

function GraphPanelInner({ planId, onClose }: GraphPanelProps) {
  const { showToast } = useToast();
  const snapshot = useGraphPlan(planId);
  const plan = snapshot.plan;
  const definition = useMemo(() => parseGraphDefinition(plan), [plan]);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [actionPending, setActionPending] = useState(false);
  const [stateOpen, setStateOpen] = useState(true);

  const planStatus = plan ? normalizePlanStatus(plan.status) : "draft";
  const statusMeta = PLAN_STATUS_META[planStatus];
  const canStart = planStatus === "draft" || planStatus === "confirmed";
  const canRestart =
    planStatus === "failed" || planStatus === "cancelled" || planStatus === "completed";
  const canCancel = planStatus === "running";

  // Esc：先关抽屉，再关面板。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (selectedNodeId) {
        setSelectedNodeId(null);
      } else {
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectedNodeId, onClose]);

  const runByNodeId = useMemo(
    () => new Map((plan?.nodeRuns ?? []).map((run) => [run.nodeId, run])),
    [plan],
  );

  const statusByNodeId = useMemo(() => {
    const map = new Map<string, GraphNodeStatus>();
    for (const node of definition?.nodes ?? []) {
      map.set(node.id, normalizeNodeStatus(runByNodeId.get(node.id)?.status ?? "pending"));
    }
    return map;
  }, [definition, runByNodeId]);

  /** 已就绪节点：自身 pending 且全部上游已成功。 */
  const readyNodeIds = useMemo(() => {
    const ready = new Set<string>();
    for (const node of definition?.nodes ?? []) {
      if (statusByNodeId.get(node.id) !== "pending") continue;
      if (node.dependsOn.every((dep) => statusByNodeId.get(dep) === "succeeded")) {
        ready.add(node.id);
      }
    }
    return ready;
  }, [definition, statusByNodeId]);

  const flowNodes = useMemo<GraphFlowNode[]>(() => {
    if (!definition) return [];
    const positions = computeGraphLayout(definition);
    return definition.nodes.map((node) => {
      const run = runByNodeId.get(node.id);
      const status = statusByNodeId.get(node.id) ?? "pending";
      return {
        id: node.id,
        type: "graphNode" as const,
        position: positions.get(node.id) ?? { x: 0, y: 0 },
        data: {
          nodeId: node.id,
          title: node.title,
          modelLabel: run?.modelLabel || node.modelRef,
          task: node.task,
          outputPreview: snapshot.liveOutputs[node.id] || run?.outputText || "",
          status,
          phase: run?.phase ?? "starting",
          durationMs: run?.durationMs ?? null,
          toolCallCount: run?.toolCallCount ?? 0,
          streaming: status === "running",
        },
      };
    });
  }, [definition, runByNodeId, snapshot.liveOutputs, statusByNodeId]);

  const flowEdges = useMemo<Edge[]>(() => {
    if (!definition) return [];
    const edges: Edge[] = [];
    for (const node of definition.nodes) {
      const targetStatus = statusByNodeId.get(node.id) ?? "pending";
      const targetReady = readyNodeIds.has(node.id);
      for (const dependency of node.dependsOn) {
        const sourceStatus = statusByNodeId.get(dependency) ?? "pending";
        const state = computeEdgeState(sourceStatus, targetStatus, targetReady);
        edges.push({
          id: `${dependency}->${node.id}`,
          source: dependency,
          target: node.id,
          // 边动画 = 数据正在流动：上游已成功且下游运行中。
          animated: state === "active",
          className: `ai-graph-edge ai-graph-edge--${state}`,
          style: { stroke: EDGE_STATE_COLOR[state] },
          markerEnd: {
            type: MarkerType.ArrowClosed,
            width: 16,
            height: 16,
            color: EDGE_STATE_COLOR[state],
          },
        });
      }
    }
    return edges;
  }, [definition, statusByNodeId, readyNodeIds]);

  // ── 头部统计：任务数 / 最大并行（最大层宽）/ 状态计数 / 整体进度 ──
  const stats = useMemo(() => {
    const nodes = definition?.nodes ?? [];
    const layers = definition ? computeGraphLayers(definition) : [];
    const maxParallel = layers.reduce((max, layer) => Math.max(max, layer.length), 0);
    const counts = { running: 0, succeeded: 0, failed: 0, settled: 0 };
    for (const node of nodes) {
      const status = statusByNodeId.get(node.id) ?? "pending";
      if (status === "running") counts.running += 1;
      if (status === "succeeded") counts.succeeded += 1;
      if (status === "failed") counts.failed += 1;
      if (status !== "pending" && status !== "running") counts.settled += 1;
    }
    const total = nodes.length;
    const progress = total > 0 ? Math.round((counts.settled / total) * 100) : 0;
    return { total, maxParallel, progress, ...counts };
  }, [definition, statusByNodeId]);

  const handleStart = useCallback(async () => {
    if (actionPending) return;
    setActionPending(true);
    try {
      await invoke("graph_run_start", { planId });
      showToast("执行图已启动");
    } catch (err) {
      showToast(`启动执行图失败：${err instanceof Error ? err.message : String(err)}`, "warning");
    } finally {
      setActionPending(false);
    }
  }, [actionPending, planId, showToast]);

  const handleCancel = useCallback(async () => {
    if (actionPending) return;
    setActionPending(true);
    try {
      await invoke<boolean>("graph_run_cancel", { planId });
    } catch (err) {
      showToast(`停止执行图失败：${err instanceof Error ? err.message : String(err)}`, "warning");
    } finally {
      setActionPending(false);
    }
  }, [actionPending, planId, showToast]);

  const selectedNodeExists = Boolean(
    selectedNodeId && definition?.nodes.some((node) => node.id === selectedNodeId),
  );

  return (
    <div className="ai-dialog-overlay ai-graph-overlay">
      <div className="ai-graph-panel" role="dialog" aria-label="执行图面板">
        <header className="ai-graph-panel-header">
          <div className="ai-graph-panel-header-top">
            <div className="ai-graph-panel-heading">
              <span className="ai-graph-panel-title">{plan?.title ?? "执行图"}</span>
              {plan?.summary && (
                <span className="ai-graph-panel-summary" title={plan.summary}>
                  {plan.summary}
                </span>
              )}
            </div>
            <span className={cn("ai-graph-chip", statusMeta.className)}>{statusMeta.label}</span>
            {canStart && (
              <Button size="sm" onClick={() => void handleStart()} disabled={actionPending || !plan}>
                <Play className="h-3.5 w-3.5" />
                确认执行
              </Button>
            )}
            {canRestart && (
              <Button
                size="sm"
                variant="outline"
                onClick={() => void handleStart()}
                disabled={actionPending || !plan}
              >
                <RotateCcw className="h-3.5 w-3.5" />
                重新执行
              </Button>
            )}
            {canCancel && (
              <Button
                size="sm"
                variant="destructive"
                onClick={() => void handleCancel()}
                disabled={actionPending}
              >
                <Square className="h-3.5 w-3.5" />
                停止
              </Button>
            )}
            <Button variant="ghost" size="icon-sm" aria-label="关闭执行图面板" onClick={onClose}>
              <X className="h-4 w-4" />
            </Button>
          </div>
          <div className="ai-graph-panel-header-stats">
            <span className="ai-graph-stat">任务 {stats.total}</span>
            <span className="ai-graph-stat">并行 {stats.maxParallel}</span>
            {stats.running > 0 && (
              <span className="ai-graph-stat ai-graph-stat--running">运行中 {stats.running}</span>
            )}
            {stats.succeeded > 0 && (
              <span className="ai-graph-stat ai-graph-stat--succeeded">成功 {stats.succeeded}</span>
            )}
            {stats.failed > 0 && (
              <span className="ai-graph-stat ai-graph-stat--failed">失败 {stats.failed}</span>
            )}
            <div className="ai-graph-progress" role="progressbar" aria-valuenow={stats.progress}>
              <div
                className={cn(
                  "ai-graph-progress-bar",
                  stats.failed > 0 && "ai-graph-progress-bar--failed",
                )}
                style={{ width: `${stats.progress}%` }}
              />
            </div>
            <span className="ai-graph-stat ai-graph-stat--progress">{stats.progress}%</span>
          </div>
        </header>

        <div className="ai-graph-panel-canvas">
          {definition && definition.nodes.length > 0 ? (
            <ReactFlow
              key={planId}
              nodes={flowNodes}
              edges={flowEdges}
              nodeTypes={nodeTypes}
              fitView
              fitViewOptions={{ padding: 0.18, maxZoom: 1.1 }}
              minZoom={0.3}
              maxZoom={1.6}
              nodesDraggable={false}
              nodesConnectable={false}
              elementsSelectable
              proOptions={{ hideAttribution: true }}
              onNodeClick={(_, node) => setSelectedNodeId(node.id)}
              onPaneClick={() => setSelectedNodeId(null)}
            >
              <Background variant={BackgroundVariant.Dots} gap={24} size={1.2} />
              <GraphCanvasControls statusByNodeId={statusByNodeId} />
              <MiniMap
                pannable
                zoomable
                className="ai-graph-minimap"
                style={{ width: 132, height: 88 }}
              />
            </ReactFlow>
          ) : (
            <div className="ai-graph-panel-empty">
              {plan ? "图定义解析失败，无法渲染画布。" : "计划加载中…"}
            </div>
          )}
        </div>

        <GraphStateInspector
          plan={plan}
          definition={definition}
          open={stateOpen}
          onToggle={() => setStateOpen((value) => !value)}
        />

        <AnimatePresence>
          {selectedNodeId && selectedNodeExists && (
            <GraphNodeDrawer
              planId={planId}
              nodeId={selectedNodeId}
              planStatus={planStatus}
              actionPending={actionPending}
              onClose={() => setSelectedNodeId(null)}
              onSelectNode={setSelectedNodeId}
              onStart={() => void handleStart()}
              onCancel={() => void handleCancel()}
            />
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}
