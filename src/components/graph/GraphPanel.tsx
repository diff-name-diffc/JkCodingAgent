import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AnimatePresence } from "framer-motion";
import {
  Background,
  BackgroundVariant,
  MarkerType,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Edge,
  type NodeChange,
  type NodeTypes,
  type Viewport,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { invoke } from "@tauri-apps/api/core";
import type { GraphNodeStatus } from "../../types";
import { DEFAULT_GRAPH_PANEL_VIEW, type GraphPanelView } from "../project/main-tabs";
import { toast } from "../Toast";
import { useWorkspaceStore } from "../../stores/workspace-store";
import { useGraphPlan } from "./graph-store";
import { computeGraphLayout, type GraphNodePosition } from "./graph-layout";
import { GraphNodeView, type GraphFlowNode } from "./GraphNodeView";
import { GraphNodeDrawer } from "./GraphNodeDrawer";
import { GraphCanvasControls } from "./GraphCanvasControls";
import { GraphPanelHeader } from "./GraphPanelHeader";
import { GraphResultView } from "./GraphResultView";
import { GraphStateInspector } from "./GraphStateInspector";
import {
  EDGE_STATE_COLOR,
  computeEdgeState,
  graphModelRefLabel,
  normalizeNodeStatus,
  normalizePlanStatus,
  parseGraphDefinition,
} from "./graph-utils";
import { useOverlayEscape } from "../../hooks/use-overlay-escape";

const nodeTypes: NodeTypes = { graphNode: GraphNodeView };

export interface GraphPanelProps {
  planId: string;
  /** 归属会话：视图记忆按会话键控，标签携带保证跨会话隔离（UI-08/13）。 */
  sessionId: string;
  /** 工作区与编辑 pane 当前是否可见（保活门控：隐藏工作区不响应快捷键）。 */
  active: boolean;
  /** 关闭主区标签。面板不再是模态覆盖层——关闭只影响视图，不影响后台执行。 */
  onClose: () => void;
  /** 返回会话图列表（两级视图的详情→列表；列表态入口在会话头部/列表行）。 */
  onBackToList?: () => void;
  /** 扩大/还原占满主区（复用会话 pane 收起机制，切换布局不触发任务重跑）。 */
  onExpandMainArea?: () => void;
  mainAreaExpanded?: boolean;
  /** 详情态初始一级视图（列表行「结果」入口传 "result" 直达执行结果）。 */
  initialView?: GraphPanelView;
  /** 打开工作区文件（执行结果视图的修改文件清单 → 主区文件标签）。 */
  onOpenFile: (path: string, name: string) => void;
}

/**
 * 执行图工作视图（UI-13）：主区标签中的普通视图，不再假扮全屏 modal
 * （portal/覆盖层栈/焦点陷阱已随迁移移除）。React Flow 画布 + 两层头部
 * + 按需共享状态检查器；选中节点/视口/手动布局按会话记忆，
 * 关标签再打开、从节点详情返回时保留（UI-14 验收依赖）。
 * 详情态内含两个平级一级视图：画布（执行图）与执行结果（结论 md +
 * 修改文件清单），经头部切换；标签的 view 字段决定初始视图。
 */
export function GraphPanel(props: GraphPanelProps) {
  // key 于 planId：切换计划整树重建，状态从视图记忆重新初始化，
  // 无旧坐标/旧选中残留（替代旧渲染阶段清空补丁）。
  return (
    <ReactFlowProvider key={props.planId}>
      <GraphPanelInner {...props} />
    </ReactFlowProvider>
  );
}

function GraphPanelInner({
  planId,
  sessionId,
  active,
  onClose,
  onBackToList,
  onExpandMainArea,
  mainAreaExpanded = false,
  initialView = DEFAULT_GRAPH_PANEL_VIEW,
  onOpenFile,
}: GraphPanelProps) {
  const { fitView } = useReactFlow();
  const snapshot = useGraphPlan(planId);
  const plan = snapshot.plan;
  const definition = useMemo(() => parseGraphDefinition(plan), [plan]);

  // 视图记忆（workspace-store 每会话临时层，不持久化）：初值只认同 planId 记录，
  // 挂载时定格（defaultViewport 等初始化 props 不随后续写回漂移）。
  const [memory] = useState(() => {
    const stored = useWorkspaceStore.getState().graphViewBySession[sessionId];
    return stored && stored.planId === planId ? stored : null;
  });
  const setGraphView = useWorkspaceStore((state) => state.setGraphView);

  const [selectedNodeId, setSelectedNodeIdState] = useState<string | null>(
    memory?.selectedNodeId ?? null,
  );
  const [actionPending, setActionPending] = useState(false);
  // 共享状态检查器默认收起（UI-13：按需显示，不再常驻侵占画布）。
  const [stateOpen, setStateOpenState] = useState(memory?.stateOpen ?? false);
  const stateOpenRef = useRef(stateOpen);
  stateOpenRef.current = stateOpen;
  /** 用户手动拖动后的节点位置覆盖（相对 dagre 自动布局）。 */
  const [dragOverrides, setDragOverrides] = useState<Record<string, GraphNodePosition>>(
    memory?.dragOverrides ?? {},
  );
  const dragOverridesRef = useRef(dragOverrides);
  dragOverridesRef.current = dragOverrides;

  const setSelectedNodeId = useCallback(
    (nodeId: string | null) => {
      setSelectedNodeIdState(nodeId);
      setGraphView(sessionId, { planId, selectedNodeId: nodeId });
    },
    [planId, sessionId, setGraphView],
  );

  const toggleStateOpen = useCallback(() => {
    const next = !stateOpenRef.current;
    setStateOpenState(next);
    setGraphView(sessionId, { planId, stateOpen: next });
  }, [planId, sessionId, setGraphView]);

  const planStatus = plan ? normalizePlanStatus(plan.status) : "draft";
  const paused = snapshot.paused;
  const canResumeRun = planStatus === "failed" || planStatus === "cancelled";

  // 详情态一级视图（画布 / 执行结果）：内部切换只改本地态；标签 view 变化
  // （列表行「结果」入口 / 意图通道）作为外部导航覆盖本地态。
  const [view, setView] = useState<GraphPanelView>(initialView);
  useEffect(() => {
    setView(initialView);
  }, [initialView]);
  /** 结果视图空态引导：跳回画布并打开指定节点抽屉。 */
  const handleOpenNodeFromResult = useCallback(
    (nodeId: string) => {
      setView("canvas");
      setSelectedNodeId(nodeId);
    },
    [setSelectedNodeId],
  );

  // Escape 只关抽屉（选中节点）；关闭面板走标签关闭语义。
  // UI-23b：抽屉打开时压入覆盖层栈——统一「栈顶裁决」使底层快捷键
  // （关 Artifact 等）让路，不再一次按键双关；隐藏工作区（active=false，
  // 保活）不注册、不占栈。
  const closeNodeDrawer = useCallback(() => setSelectedNodeId(null), [setSelectedNodeId]);
  useOverlayEscape("graph-node-drawer", active && Boolean(selectedNodeId), closeNodeDrawer);

  // 保活切回 / pane 重显：无手动视口记忆时重新适应画布（有记忆则尊重用户位置）。
  const prevActiveRef = useRef(active);
  useEffect(() => {
    const becameActive = active && !prevActiveRef.current;
    prevActiveRef.current = active;
    if (!becameActive) return;
    const current = useWorkspaceStore.getState().graphViewBySession[sessionId];
    if (current?.planId === planId && current.viewport) return;
    const timer = window.setTimeout(() => void fitView({ padding: 0.18, maxZoom: 1 }), 60);
    return () => window.clearTimeout(timer);
  }, [active, fitView, planId, sessionId]);

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
          modelLabel: run?.modelLabel || graphModelRefLabel(node.modelRef),
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

  /** 受控节点：吸收拖动位置；拖拽结束（dragging=false）才写入视图记忆，不高频打 store。 */
  const handleNodesChange = useCallback(
    (changes: NodeChange[]) => {
      const moved = changes.filter(
        (change): change is Extract<NodeChange, { type: "position" }> =>
          change.type === "position" && change.position != null,
      );
      if (moved.length === 0) return;
      const next = { ...dragOverridesRef.current };
      for (const change of moved) {
        next[change.id] = change.position as GraphNodePosition;
      }
      dragOverridesRef.current = next;
      setDragOverrides(next);
      if (moved.some((change) => change.dragging === false)) {
        setGraphView(sessionId, { planId, dragOverrides: next });
      }
    },
    [planId, sessionId, setGraphView],
  );

  /** 视口记忆：平移/缩放结束写入（onMoveEnd 天然低频）。 */
  const handleMoveEnd = useCallback(
    (_event: MouseEvent | TouchEvent | null, viewport: Viewport) => {
      setGraphView(sessionId, {
        planId,
        viewport: { x: viewport.x, y: viewport.y, zoom: viewport.zoom },
      });
    },
    [planId, sessionId, setGraphView],
  );

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
        toast.success(mode === "resume" ? "已从断点继续执行" : "执行图已启动");
      } catch (err) {
        toast.warning(`启动执行图失败：${err instanceof Error ? err.message : String(err)}`);
      } finally {
        setActionPending(false);
      }
    },
    [actionPending, planId],
  );

  const handleResumeCheckpoint = useCallback(async () => {
    if (actionPending) return;
    setActionPending(true);
    try {
      // 后端返回 false 表示当前没有可恢复的暂停运行（无活跃 run 条目），
      // 不能当成成功提示，否则会掩盖恢复未生效的事实。
      const resumed = await invoke<boolean>("graph_run_resume", { planId });
      if (resumed) {
        toast.success("已恢复执行");
      } else {
        toast.warning("当前没有可恢复的暂停运行");
      }
    } catch (err) {
      toast.warning(`恢复执行失败：${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setActionPending(false);
    }
  }, [actionPending, planId]);

  const handleCancel = useCallback(async () => {
    if (actionPending) return;
    setActionPending(true);
    try {
      await invoke<boolean>("graph_run_cancel", { planId });
    } catch (err) {
      toast.warning(`停止执行图失败：${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setActionPending(false);
    }
  }, [actionPending, planId]);

  // 重新验收最近一次已收尾的运行：最长一次模型调用（后端 90s 超时），用独立
  // pending 态不占用 actionPending——验收期间不应禁掉「完整重跑」等计划操作。
  const [reverifyPending, setReverifyPending] = useState(false);
  const handleReverify = useCallback(async () => {
    const runId = plan?.runs?.[0]?.id;
    if (!runId || reverifyPending) return;
    setReverifyPending(true);
    try {
      await invoke("graph_run_reverify", { runId });
      // 结论随 graph-plan-updated 事件刷新（回执也会写回聊天流），这里只提示完成。
      toast.success("重新验收完成");
    } catch (err) {
      toast.warning(`重新验收失败：${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setReverifyPending(false);
    }
  }, [plan, reverifyPending]);

  const selectedNodeExists = Boolean(
    selectedNodeId && definition?.nodes.some((node) => node.id === selectedNodeId),
  );

  return (
    <div className="ai-graph-panel" role="region" aria-label="执行图">
      <GraphPanelHeader
        plan={plan}
        definition={definition}
        planStatus={planStatus}
        paused={paused}
        actionPending={actionPending}
        statusByNodeId={statusByNodeId}
        view={view}
        onViewChange={setView}
        onStart={(mode) => void handleStart(mode)}
        onResumeCheckpoint={() => void handleResumeCheckpoint()}
        onCancel={() => void handleCancel()}
        onReverify={() => void handleReverify()}
        reverifyPending={reverifyPending}
        onClose={onClose}
        onBackToList={onBackToList}
        onExpandMainArea={onExpandMainArea}
        mainAreaExpanded={mainAreaExpanded}
      />

      {view === "result" ? (
        <GraphResultView
          plan={plan}
          onOpenNode={handleOpenNodeFromResult}
          onOpenFile={onOpenFile}
        />
      ) : (
        <>
          <div className="ai-graph-panel-canvas">
            {definition && definition.nodes.length > 0 ? (
              <ReactFlow
                nodes={flowNodes}
                edges={flowEdges}
                nodeTypes={nodeTypes}
                onNodesChange={handleNodesChange}
                onMoveEnd={handleMoveEnd}
                {...(memory?.viewport
                  ? { defaultViewport: memory.viewport }
                  : { fitView: true, fitViewOptions: { padding: 0.18, maxZoom: 1 } })}
                // maxZoom 限制为 1：CSS transform 放大文本会明显发虚
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
                  onResetLayout={() => {
                    setDragOverrides({});
                    dragOverridesRef.current = {};
                    setGraphView(sessionId, { planId, dragOverrides: {} });
                  }}
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
            onToggle={toggleStateOpen}
          />
        </>
      )}

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
  );
}
