import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AnimatePresence } from "framer-motion";
import {
  Background,
  BackgroundVariant,
  MarkerType,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  type Edge,
  type NodeChange,
  type NodeTypes,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { invoke } from "@tauri-apps/api/core";
import type { GraphNodeStatus } from "../../types";
import { useToast } from "../Toast";
import { useGraphPlan } from "./graph-store";
import { computeGraphLayout, type GraphNodePosition } from "./graph-layout";
import { GraphNodeView, type GraphFlowNode } from "./GraphNodeView";
import { GraphNodeDrawer } from "./GraphNodeDrawer";
import { GraphCanvasControls } from "./GraphCanvasControls";
import { GraphPanelHeader } from "./GraphPanelHeader";
import { GraphStateInspector } from "./GraphStateInspector";
import {
  EDGE_STATE_COLOR,
  computeEdgeState,
  normalizeNodeStatus,
  normalizePlanStatus,
  parseGraphDefinition,
} from "./graph-utils";
import { isTopOverlay, popOverlay, pushOverlay } from "../../lib/overlay-stack";

const nodeTypes: NodeTypes = { graphNode: GraphNodeView };

/** 覆盖层栈中的身份标识（Escape 层级协调用）。 */
const GRAPH_OVERLAY_ID = "graph-panel";

export interface GraphPanelProps {
  planId: string;
  onClose: () => void;
}

/**
 * 图编排全屏面板：React Flow 画布 + 两层头部（GraphPanelHeader）
 * + 底部共享 state 检查器。任何状态都可关闭面板（关闭不影响后台执行）。
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
  /** 用户手动拖动后的节点位置覆盖（相对 dagre 自动布局）。 */
  const [dragOverrides, setDragOverrides] = useState<Record<string, GraphNodePosition>>({});
  const [overridesScope, setOverridesScope] = useState(planId);
  if (overridesScope !== planId) {
    // 切换计划时在渲染阶段同步清空（而非 useEffect——它在绘制后才执行，
    // 切换瞬间会先用旧覆盖位置渲染一帧；各计划节点 id 形如 n1/n2 高度
    // 重合，旧覆盖会直接命中新计划的同名节点）。
    setOverridesScope(planId);
    setDragOverrides({});
  }

  const planStatus = plan ? normalizePlanStatus(plan.status) : "draft";
  const paused = snapshot.paused;
  const canResumeRun = planStatus === "failed" || planStatus === "cancelled";

  // Esc：先关抽屉，再关面板。仅覆盖层栈顶响应且标记 defaultPrevented，
  // 避免一次按键同时关掉多层（自研覆盖层与底层快捷键的协调见 overlay-stack）。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      if (!isTopOverlay(GRAPH_OVERLAY_ID)) return;
      event.preventDefault();
      if (selectedNodeId) {
        setSelectedNodeId(null);
      } else {
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectedNodeId, onClose]);

  const panelRef = useRef<HTMLDivElement | null>(null);

  // 层级登记 + 焦点止损：打开时入栈并接管焦点，关闭后还原到触发元素。
  useEffect(() => {
    const trigger =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    pushOverlay(GRAPH_OVERLAY_ID);
    panelRef.current?.focus();
    return () => {
      popOverlay(GRAPH_OVERLAY_ID);
      trigger?.focus();
    };
  }, []);

  // 焦点陷阱：Tab 循环限制在面板内，避免焦点走进被遮挡的底层界面。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Tab" || !isTopOverlay(GRAPH_OVERLAY_ID)) return;
      const root = panelRef.current;
      if (!root) return;
      const focusables = Array.from(
        root.querySelectorAll<HTMLElement>(
          'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
        ),
      ).filter((el) => !el.hasAttribute("disabled"));
      if (focusables.length === 0) return;
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement;
      if (event.shiftKey && (active === first || active === root)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && active === last) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

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
        position: dragOverrides[node.id] ?? positions.get(node.id) ?? { x: 0, y: 0 },
        // 选中态由抽屉打开的节点推导，避免受控模式下内部选中状态丢失
        selected: node.id === selectedNodeId,
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
  }, [definition, runByNodeId, snapshot.liveOutputs, statusByNodeId, dragOverrides, selectedNodeId]);

  /** 受控节点：仅吸收拖动产生的位置变化，写回覆盖层。 */
  const handleNodesChange = useCallback((changes: NodeChange[]) => {
    const moved = changes.filter(
      (change): change is Extract<NodeChange, { type: "position" }> =>
        change.type === "position" && change.position != null,
    );
    if (moved.length === 0) return;
    setDragOverrides((prev) => {
      const next = { ...prev };
      for (const change of moved) {
        next[change.id] = change.position as GraphNodePosition;
      }
      return next;
    });
  }, []);

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

  // ── 头部统计与验收结论在 GraphPanelHeader 内计算 ──

  const handleStart = useCallback(
    async (mode: "full" | "resume") => {
      if (actionPending) return;
      setActionPending(true);
      try {
        await invoke("graph_run_start", { planId, mode });
        showToast(mode === "resume" ? "已从断点继续执行" : "执行图已启动");
      } catch (err) {
        showToast(`启动执行图失败：${err instanceof Error ? err.message : String(err)}`, "warning");
      } finally {
        setActionPending(false);
      }
    },
    [actionPending, planId, showToast],
  );

  const handleResumeCheckpoint = useCallback(async () => {
    if (actionPending) return;
    setActionPending(true);
    try {
      // 后端返回 false 表示当前没有可恢复的暂停运行（无活跃 run 条目），
      // 不能当成成功提示，否则会掩盖恢复未生效的事实。
      const resumed = await invoke<boolean>("graph_run_resume", { planId });
      if (resumed) {
        showToast("已恢复执行");
      } else {
        showToast("当前没有可恢复的暂停运行", "warning");
      }
    } catch (err) {
      showToast(`恢复执行失败：${err instanceof Error ? err.message : String(err)}`, "warning");
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

  // portal 到 body：项目页 .ai-project-shell > * 会给祖先创建层叠上下文，
  // 内联渲染时图面板会被右侧文件栏遮挡（A02）；portal 后 z-index 在根层级生效。
  return createPortal(
    <div className="ai-dialog-overlay ai-graph-overlay ai-graph-portal">
      <div
        ref={panelRef}
        tabIndex={-1}
        className="ai-graph-panel"
        role="dialog"
        aria-modal="true"
        aria-label="执行图面板"
      >
        <GraphPanelHeader
          plan={plan}
          definition={definition}
          planStatus={planStatus}
          paused={paused}
          actionPending={actionPending}
          statusByNodeId={statusByNodeId}
          onStart={(mode) => void handleStart(mode)}
          onResumeCheckpoint={() => void handleResumeCheckpoint()}
          onCancel={() => void handleCancel()}
          onClose={onClose}
        />

        <div className="ai-graph-panel-canvas">
          {definition && definition.nodes.length > 0 ? (
            <ReactFlow
              key={planId}
              nodes={flowNodes}
              edges={flowEdges}
              nodeTypes={nodeTypes}
              onNodesChange={handleNodesChange}
              fitView
              // maxZoom 限制为 1：CSS transform 放大文本会明显发虚
              fitViewOptions={{ padding: 0.18, maxZoom: 1 }}
              minZoom={0.3}
              maxZoom={2}
              nodesDraggable
              nodesConnectable={false}
              elementsSelectable
              proOptions={{ hideAttribution: true }}
              onNodeClick={(_, node) => setSelectedNodeId(node.id)}
              onPaneClick={() => setSelectedNodeId(null)}
            >
              <Background variant={BackgroundVariant.Dots} gap={24} size={1.2} />
              <GraphCanvasControls
                statusByNodeId={statusByNodeId}
                hasCustomLayout={Object.keys(dragOverrides).length > 0}
                onResetLayout={() => setDragOverrides({})}
              />
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
              onStart={() => void handleStart(canResumeRun ? "resume" : "full")}
              onCancel={() => void handleCancel()}
            />
          )}
        </AnimatePresence>
      </div>
    </div>,
    document.body,
  );
}
