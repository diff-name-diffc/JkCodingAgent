import { useEffect, useMemo, useRef, useState } from "react";
import { motion } from "framer-motion";
import { invoke } from "@tauri-apps/api/core";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Activity, BrainCircuit, ChevronDown, ChevronRight, ChevronsDown, Clock3, Pause, Play, RotateCcw, Square, Wrench, X } from "lucide-react";
import type { AgentActivity, GraphBaseToolGroup, GraphDefinition, GraphHarnessCatalog, GraphPlanStatus, GraphRunDetail } from "../../types";
import { cn } from "../../lib/cn";
import { useToast } from "../Toast";
import { Button } from "../ui/button";
import { Textarea } from "../ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../ui/select";
import { createSerialTaskQueue, hydrateGraphPlan, useGraphPlan } from "./graph-store";
import { NODE_STATUS_META, formatGraphDuration, normalizeNodeStatus, parseGraphDefinition } from "./graph-utils";

interface GraphNodeDrawerProps {
  planId: string;
  nodeId: string;
  planStatus: GraphPlanStatus;
  actionPending: boolean;
  onClose: () => void;
  onSelectNode: (nodeId: string) => void;
  onStart: () => void;
  onCancel: () => void;
}

const ACTIVITY_LABEL: Record<string, string> = {
  assistant_text: "Agent 响应", thinking: "思考", tool_call: "工具调用",
  retry: "自动重试", compaction: "上下文压缩", usage: "Token 用量", error: "错误",
};

export function GraphNodeDrawer(props: GraphNodeDrawerProps) {
  const { planId, nodeId, planStatus, actionPending, onClose, onSelectNode, onStart, onCancel } = props;
  const { showToast } = useToast();
  const snapshot = useGraphPlan(planId);
  const plan = snapshot.plan;
  const definition = useMemo(() => parseGraphDefinition(plan), [plan]);
  const node = definition?.nodes.find((item) => item.id === nodeId) ?? null;
  const [selectedRunId, setSelectedRunId] = useState<string>(plan?.latestRunId ?? "");
  const [runDetail, setRunDetail] = useState<GraphRunDetail | null>(null);
  const [catalog, setCatalog] = useState<GraphHarnessCatalog | null>(null);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [draftTask, setDraftTask] = useState(node?.task ?? "");
  const [saving, setSaving] = useState(false);
  const [enqueueSave] = useState(() => createSerialTaskQueue());
  const pendingSaves = useRef(0);

  useEffect(() => { setSelectedRunId(plan?.latestRunId ?? ""); }, [plan?.latestRunId]);
  useEffect(() => { setDraftTask(node?.task ?? ""); }, [node?.task, nodeId]);
  useEffect(() => {
    if (!selectedRunId) { setRunDetail(null); return; }
    let alive = true;
    invoke<GraphRunDetail>("graph_run_get", { runId: selectedRunId })
      .then((detail) => { if (alive) setRunDetail(detail); })
      .catch((error) => { if (alive) showToast(`加载运行详情失败：${String(error)}`, "warning"); });
    return () => { alive = false; };
  }, [selectedRunId, showToast]);
  useEffect(() => {
    if (plan?.status !== "draft" || !plan.workspaceId) return;
    invoke<GraphHarnessCatalog>("graph_harness_catalog_get", { workspaceId: plan.workspaceId })
      .then(setCatalog)
      .catch((error) => showToast(`加载 Harness 目录失败：${String(error)}`, "warning"));
  }, [plan?.status, plan?.workspaceId, showToast]);

  const historicalRun = runDetail?.nodeRuns.find((item) => item.nodeId === nodeId) ?? null;
  const currentRun = plan?.nodeRuns.find((item) => item.nodeId === nodeId) ?? null;
  const nodeRun = selectedRunId === plan?.latestRunId ? currentRun : historicalRun;
  const liveOutput = selectedRunId === plan?.latestRunId ? snapshot.liveOutputs[nodeId] ?? "" : "";
  const output = liveOutput || nodeRun?.outputText || "";
  const historicalActivities = runDetail?.activities.filter((item) => item.nodeId === nodeId) ?? [];
  const liveActivities = selectedRunId === plan?.latestRunId ? snapshot.liveActivities[nodeId] ?? [] : [];
  const activities = liveActivities.length > 0 ? liveActivities : historicalActivities;
  const status = normalizeNodeStatus(nodeRun?.status ?? "pending");
  const statusMeta = NODE_STATUS_META[status];
  const editable = plan?.status === "draft" && Boolean(node && definition);

  async function updateDefinition(mutator: (definition: GraphDefinition) => GraphDefinition): Promise<boolean> {
    if (!definition || !editable) return false;
    pendingSaves.current += 1;
    setSaving(true);
    try {
      await enqueueSave(async () => {
        const latestPlan = await hydrateGraphPlan(planId);
        const latestDefinition = parseGraphDefinition(latestPlan);
        if (!latestPlan || latestPlan.status !== "draft" || !latestDefinition) {
          throw new Error("图计划已不可编辑或最新定义加载失败");
        }
        await invoke("graph_plan_update", { planId, definitionJson: JSON.stringify(mutator(latestDefinition)) });
        if (!await hydrateGraphPlan(planId)) throw new Error("保存后重新加载图计划失败");
      });
      return true;
    } catch (error) {
      showToast(`保存节点配置失败：${error instanceof Error ? error.message : String(error)}`, "warning");
      return false;
    } finally {
      pendingSaves.current -= 1;
      setSaving(pendingSaves.current > 0);
    }
  }

  async function patchNode(patch: Partial<NonNullable<typeof node>>): Promise<boolean> {
    return updateDefinition((value) => ({ ...value, nodes: value.nodes.map((item) => item.id === nodeId ? { ...item, ...patch } : item) }));
  }

  async function toggleTool(source: "aha" | "pi_extension", name: string): Promise<boolean> {
    return updateDefinition((value) => ({
      ...value,
      nodes: value.nodes.map((item) => {
        if (item.id !== nodeId) return item;
        const selected = item.specialTools.some((tool) => tool.source === source && tool.name === name);
        return {
          ...item,
          specialTools: selected
            ? item.specialTools.filter((tool) => tool.source !== source || tool.name !== name)
            : [...item.specialTools, { source, name }],
        };
      }),
    }));
  }

  const upstream = node?.dependsOn ?? [];
  const downstream = definition?.nodes.filter((item) => item.dependsOn.includes(nodeId)).map((item) => item.id) ?? [];
  const titleOf = (id: string) => definition?.nodes.find((item) => item.id === id)?.title ?? id;

  return (
    <motion.aside initial={{ x: 460, opacity: 0 }} animate={{ x: 0, opacity: 1 }} exit={{ x: 460, opacity: 0 }} transition={{ duration: 0.2 }} className="ai-graph-drawer">
      <div className="ai-graph-drawer-header">
        <span className="ai-graph-drawer-agent ai-graph-drawer-agent--pi"><BrainCircuit className="h-3.5 w-3.5" />PI Agent</span>
        <span className={cn("ai-graph-chip", `ai-graph-chip--node-${status}`)}>{statusMeta.label}</span>
        {nodeRun?.durationMs != null && <span className="ai-graph-drawer-duration"><Clock3 className="h-3 w-3" />{formatGraphDuration(nodeRun.durationMs)}</span>}
        <Button variant="ghost" size="icon-sm" aria-label="关闭节点详情" onClick={onClose}><X className="h-4 w-4" /></Button>
      </div>

      <div className="ai-graph-drawer-body">
        <div className="ai-graph-drawer-title-row">
          <h3 className="ai-graph-drawer-title">{node?.title ?? nodeId}</h3>
          {plan && plan.runs.length > 0 && (
            <Select value={selectedRunId} onValueChange={setSelectedRunId}>
              <SelectTrigger className="h-8 w-36 text-xs"><SelectValue placeholder="选择运行" /></SelectTrigger>
              <SelectContent>{plan.runs.map((run) => <SelectItem key={run.id} value={run.id}>第 {run.attemptNo} 次 · {run.status}</SelectItem>)}</SelectContent>
            </Select>
          )}
        </div>

        {editable && catalog && node && (
          <section className="ai-graph-drawer-section ai-graph-harness-editor">
            <div className="ai-graph-drawer-label">运行 Harness {saving && <span className="ai-graph-drawer-hint">保存中…</span>}</div>
            <Select value={node.modelRef} onValueChange={(modelRef) => void patchNode({ modelRef })} disabled={saving}>
              <SelectTrigger><SelectValue placeholder="选择主模型" /></SelectTrigger>
              <SelectContent>{catalog.models.map((model) => <SelectItem key={model.id} value={model.id}>{model.label} · {model.category}{model.capabilities.length > 0 ? ` · ${model.capabilities.join("/")}` : ""}</SelectItem>)}</SelectContent>
            </Select>
            <Select value={node.baseToolGroup} onValueChange={(baseToolGroup) => void patchNode({ baseToolGroup: baseToolGroup as GraphBaseToolGroup })} disabled={saving}>
              <SelectTrigger><SelectValue /></SelectTrigger>
              <SelectContent><SelectItem value="read_only">只读 · read/grep/find/ls</SelectItem><SelectItem value="coding">编码 · 加 bash/edit/write</SelectItem></SelectContent>
            </Select>
            <div className="ai-graph-tool-picker">
              {catalog.tools.map((tool) => {
                const selected = node.specialTools.some((item) => item.source === tool.source && item.name === tool.name);
                const safety = tool.reviewRequired ? "需审查" : tool.readonly ? "只读" : "直接执行";
                return <button key={`${tool.source}:${tool.name}`} type="button" className={cn("ai-graph-tool-toggle", selected && "is-selected")} title={`${tool.description}\n${tool.provider} · ${safety}`} disabled={saving} onClick={() => void toggleTool(tool.source, tool.name)}><Wrench className="h-3 w-3" /><span>{tool.name}</span><span className="ai-graph-tool-source">{tool.source === "aha" ? "Aha" : "PI"} · {safety}</span></button>;
              })}
            </div>
            {catalog.diagnostics.length > 0 && <div className="ai-graph-catalog-diagnostics">{catalog.diagnostics.map((diagnostic) => <div key={diagnostic}>{diagnostic}</div>)}</div>}
          </section>
        )}

        <section className="ai-graph-drawer-section">
          <div className="ai-graph-drawer-label">Agent 输入 {editable && <span className="ai-graph-drawer-hint">可编辑任务，失焦保存</span>}</div>
          {editable ? <Textarea value={draftTask} onChange={(event) => setDraftTask(event.target.value)} onBlur={() => { const task = draftTask.trim(); if (task && task !== node?.task) void patchNode({ task }).then((saved) => { if (!saved) setDraftTask(node?.task ?? ""); }); }} rows={8} className="resize-y font-mono text-xs leading-relaxed" /> : <pre className="ai-graph-drawer-pre">{nodeRun?.inputText || node?.task || "等待运行"}</pre>}
        </section>

        <section className="ai-graph-drawer-section ai-graph-output-section">
          <div className="ai-graph-drawer-label">{liveOutput ? "实时响应" : "Agent 输出"}</div>
          <pre className="ai-graph-drawer-pre ai-graph-drawer-pre--output">{output || (status === "running" ? "PI Agent 正在准备…" : "尚无输出")}</pre>
        </section>

        {(upstream.length > 0 || downstream.length > 0) && <section className="ai-graph-drawer-section"><div className="ai-graph-drawer-label">依赖关系</div><div className="ai-graph-drawer-deps-chips">{[...upstream, ...downstream].map((id) => <button key={id} className="ai-graph-dep-chip" onClick={() => onSelectNode(id)}>{titleOf(id)}</button>)}</div></section>}

        <section className="ai-graph-drawer-section">
          <button type="button" className="ai-graph-drawer-label ai-graph-drawer-label--toggle" onClick={() => setDetailsOpen((value) => !value)} aria-expanded={detailsOpen}>{detailsOpen ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}执行详情 <span className="ai-graph-drawer-hint">{activities.length} 条活动</span></button>
          {detailsOpen && <VirtualActivityList activities={activities} />}
        </section>

        {nodeRun?.errorText && <section className="ai-graph-drawer-section"><div className="ai-graph-drawer-label">错误</div><div className="ai-graph-drawer-error">{nodeRun.errorText}</div></section>}
      </div>

      {(planStatus === "running" || ["failed", "cancelled", "completed"].includes(planStatus)) && <div className="ai-graph-drawer-footer">{planStatus === "running" ? <Button variant="destructive" size="sm" onClick={onCancel} disabled={actionPending}><Square className="h-3.5 w-3.5" />停止</Button> : <Button variant="outline" size="sm" onClick={onStart} disabled={actionPending}>{planStatus === "completed" ? <RotateCcw className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}重新执行</Button>}</div>}
    </motion.aside>
  );
}

function VirtualActivityList({ activities }: { activities: AgentActivity[] }) {
  const ordered = useMemo(
    () => [...activities].sort((left, right) => left.sequence - right.sequence),
    [activities],
  );
  const scrollRef = useRef<HTMLDivElement>(null);
  const [following, setFollowing] = useState(true);
  const virtualizer = useVirtualizer({
    count: ordered.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 72,
    overscan: 6,
  });

  useEffect(() => {
    if (following && ordered.length > 0) {
      virtualizer.scrollToIndex(ordered.length - 1, { align: "end" });
    }
  }, [following, ordered.length, virtualizer]);

  if (ordered.length === 0) {
    return <p className="ai-graph-drawer-hint">尚未记录 Agent 活动。</p>;
  }
  return (
    <div className="ai-graph-activity-shell">
      <button type="button" className="ai-graph-follow-toggle" onClick={() => setFollowing((value) => !value)}>
        {following ? <Pause className="h-3 w-3" /> : <ChevronsDown className="h-3 w-3" />}
        {following ? "暂停跟随" : "恢复跟随"}
      </button>
      <div
        ref={scrollRef}
        className="ai-graph-activity-list"
        onScroll={(event) => {
          const target = event.currentTarget;
          if (target.scrollHeight - target.scrollTop - target.clientHeight > 40) setFollowing(false);
        }}
      >
        <div className="ai-graph-activity-virtual" style={{ height: virtualizer.getTotalSize() }}>
          {virtualizer.getVirtualItems().map((item) => (
            <div
              key={ordered[item.index].id}
              ref={virtualizer.measureElement}
              data-index={item.index}
              className="ai-graph-activity-virtual-row"
              style={{ transform: `translateY(${item.start}px)` }}
            >
              <ActivityRow activity={ordered[item.index]} />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function ActivityRow({ activity }: { activity: AgentActivity }) {
  const [open, setOpen] = useState(activity.kind === "tool_call");
  return <div className={cn("ai-graph-activity", `ai-graph-activity--${activity.kind}`)}><button type="button" className="ai-graph-activity-head" onClick={() => setOpen((value) => !value)}>{open ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}<Activity className="h-3 w-3" /><span>{activity.title || ACTIVITY_LABEL[activity.kind] || activity.kind}</span><span className="ai-graph-activity-status">{activity.status}</span></button>{open && <pre className="ai-graph-activity-content">{activity.content || activity.payloadJson}</pre>}</div>;
}
