import { memo, useEffect, useMemo, useRef, useState } from "react";
import { motion } from "framer-motion";
import { invoke } from "@tauri-apps/api/core";
import { useVirtualizer } from "@tanstack/react-virtual";
import { BrainCircuit, ChevronDown, ChevronRight, ChevronsDown, Clock3, Gauge, Pause, Play, RotateCcw, Shrink, Square, Wrench, X } from "lucide-react";
import type { AgentActivity, GraphBaseToolGroup, GraphDefinition, GraphHarnessCatalog, GraphPlanStatus, GraphRunDetail } from "../../types";
import { cn } from "../../lib/cn";
import { useToast } from "../Toast";
import { Button } from "../ui/button";
import { Textarea } from "../ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../ui/select";
import { createSerialTaskQueue, hydrateGraphPlan, useGraphPlan } from "./graph-store";
import {
  NODE_STATUS_META,
  buildExecutionTimeline,
  formatCharCount,
  formatContextUsage,
  formatGraphDuration,
  normalizeNodeStatus,
  parseGraphDefinition,
  type NodeNotice,
  type TimelineRow,
  type ToolCallEntry,
} from "./graph-utils";

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
  // 输入区默认收起、输出区默认展开（执行时以工具调用列表为主视图）
  const [inputOpen, setInputOpen] = useState(false);
  const [outputOpen, setOutputOpen] = useState(true);
  const [draftTask, setDraftTask] = useState(node?.task ?? "");
  const [draftExpectedFiles, setDraftExpectedFiles] = useState((node?.expectedFiles ?? []).join(", "));
  const [saving, setSaving] = useState(false);
  const [enqueueSave] = useState(() => createSerialTaskQueue());
  const pendingSaves = useRef(0);

  useEffect(() => { setSelectedRunId(plan?.latestRunId ?? ""); }, [plan?.latestRunId]);
  // 切换节点时重置折叠状态：组件常驻挂载（不随 nodeId 重建），
  // 否则会从上一节点继承输入/输出区的展开收起状态
  useEffect(() => { setInputOpen(false); setOutputOpen(true); }, [nodeId]);
  useEffect(() => { setDraftTask(node?.task ?? ""); }, [node?.task, nodeId]);
  useEffect(() => { setDraftExpectedFiles((node?.expectedFiles ?? []).join(", ")); }, [node?.expectedFiles, nodeId]);
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
  // 历史 activities 用 useMemo 稳定引用：行内 filter 每次渲染都新建数组，
  // 会让下方派生的 useMemo 依赖恒变，查看历史运行（无实时 activities）时
  // 跟随滚动/snapshot 更新引发的每次重渲染都会重复执行派生（含 JSON.parse
  // 与格式化）。空回退统一用模块级常量，避免 `?? []` 每次生成新空数组。
  const historicalActivities = useMemo(
    () => runDetail?.activities.filter((item) => item.nodeId === nodeId) ?? EMPTY_ACTIVITIES,
    [runDetail, nodeId],
  );
  const liveActivities = selectedRunId === plan?.latestRunId
    ? snapshot.liveActivities[nodeId] ?? EMPTY_ACTIVITIES
    : EMPTY_ACTIVITIES;
  const activities = liveActivities.length > 0 ? liveActivities : historicalActivities;
  // 工具条目、时间线行与上下文读数共享一次派生，避免 buildToolCallEntries 重复执行。
  const { toolEntries, timelineRows, contextUsage } = useMemo(
    () => buildExecutionTimeline(activities),
    [activities],
  );
  const toolTotals = useMemo(() => {
    let input = 0;
    let outputChars = 0;
    for (const entry of toolEntries) {
      input += entry.inputChars;
      outputChars += entry.outputChars;
    }
    return { input, output: outputChars };
  }, [toolEntries]);
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

  // 草稿提交：失焦与「切换节点/关闭抽屉前的兜底 flush」共用，逻辑保持单一。
  function commitDraftTask(): void {
    const task = draftTask.trim();
    if (!task || task === node?.task) return;
    void patchNode({ task }).then((saved) => { if (!saved) setDraftTask(node?.task ?? ""); });
  }

  function commitDraftExpectedFiles(): void {
    const files = draftExpectedFiles.split(",").map((file) => file.trim()).filter(Boolean);
    const current = node?.expectedFiles ?? [];
    if (files.length === current.length && files.every((file, index) => file === current[index])) return;
    void patchNode({ expectedFiles: files }).then((saved) => { if (!saved) setDraftExpectedFiles(current.join(", ")); });
  }

  // 切换节点 / 抽屉卸载（关闭抽屉、关闭面板）前兜底提交未失焦的草稿：
  // 依赖 chips 切换与关闭都不会触发 onBlur，不 flush 会静默丢失编辑。
  // ref 在无依赖 effect 中更新——同一提交阶段 cleanup 先于 setup 执行，
  // 因此 cleanup 时读到的仍是「切换前」那次渲染的快照（旧节点 + 旧草稿）。
  const flushDraftsRef = useRef<() => void>(() => {});
  useEffect(() => {
    flushDraftsRef.current = () => {
      if (!editable) return;
      commitDraftTask();
      commitDraftExpectedFiles();
    };
  });
  useEffect(() => {
    return () => flushDraftsRef.current();
  }, [nodeId]);

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
  const inputText = nodeRun?.inputText || node?.task || "";
  // 超长输出只渲染尾部最新部分，避免 MB 级文本拖慢渲染
  const outputDisplay = output.length > OUTPUT_DISPLAY_LIMIT ? output.slice(-OUTPUT_DISPLAY_LIMIT) : output;
  const outputOmitted = output.length - outputDisplay.length;

  return (
    <motion.aside initial={{ x: 460, opacity: 0 }} animate={{ x: 0, opacity: 1 }} exit={{ x: 460, opacity: 0 }} transition={{ duration: 0.2 }} className="ai-graph-drawer">
      <div className="ai-graph-drawer-header">
        <span className="ai-graph-drawer-agent"><BrainCircuit className="h-3.5 w-3.5" />PI Agent</span>
        <span className={cn("ai-graph-chip", `ai-graph-chip--node-${status}`)}>{statusMeta.label}</span>
        {nodeRun?.durationMs != null && <span className="ai-graph-drawer-duration"><Clock3 className="h-3 w-3" />{formatGraphDuration(nodeRun.durationMs)}</span>}
        {contextUsage && (
          <span className="ai-graph-drawer-usage" title="上下文窗口占用（PI 运行时估算值）">
            <Gauge className="h-3 w-3" />{formatContextUsage(contextUsage)}
          </span>
        )}
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
            <Select value={node.exportPolicy ?? "summary"} onValueChange={(exportPolicy) => void patchNode({ exportPolicy: exportPolicy as "summary" | "full" })} disabled={saving}>
              <SelectTrigger><SelectValue placeholder="下游导出策略" /></SelectTrigger>
              <SelectContent><SelectItem value="summary">下游只见产出摘要</SelectItem><SelectItem value="full">下游可见完整输出</SelectItem></SelectContent>
            </Select>
            <input
              type="text"
              className="ai-graph-expected-files"
              placeholder="预期读写文件（逗号分隔，供并行写冲突预检）"
              value={draftExpectedFiles}
              disabled={saving}
              onChange={(event) => setDraftExpectedFiles(event.target.value)}
              onBlur={commitDraftExpectedFiles}
            />
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
          <div className="ai-graph-drawer-label">
            执行过程
            <span className="ai-graph-drawer-hint">
              {executionHint(toolEntries.length, timelineRows.length, status, toolTotals)}
            </span>
          </div>
          <ExecutionTimelineList rows={timelineRows} live={status === "running"} />
        </section>

        <section className="ai-graph-drawer-section">
          {editable ? (
            <>
              <div className="ai-graph-drawer-label">Agent 输入 <span className="ai-graph-drawer-hint">可编辑任务，失焦保存</span></div>
              <Textarea value={draftTask} onChange={(event) => setDraftTask(event.target.value)} onBlur={commitDraftTask} rows={8} className="resize-y font-mono text-xs leading-relaxed" />
            </>
          ) : (
            <>
              <button type="button" className="ai-graph-drawer-label ai-graph-drawer-label--toggle" onClick={() => setInputOpen((value) => !value)} aria-expanded={inputOpen}>
                {inputOpen ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
                Agent 输入
                <span className="ai-graph-drawer-hint">{inputText ? `${formatCharCount(inputText.length)} 字符` : "等待运行"}</span>
              </button>
              {inputOpen && <pre className="ai-graph-drawer-pre">{inputText || "等待运行"}</pre>}
            </>
          )}
        </section>

        <section className="ai-graph-drawer-section ai-graph-output-section">
          <button type="button" className="ai-graph-drawer-label ai-graph-drawer-label--toggle" onClick={() => setOutputOpen((value) => !value)} aria-expanded={outputOpen}>
            {outputOpen ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
            {liveOutput ? "实时响应" : "Agent 输出"}
            {output && <span className="ai-graph-drawer-hint">{formatCharCount(output.length)} 字符</span>}
          </button>
          {outputOpen && (
            <pre className="ai-graph-drawer-pre ai-graph-drawer-pre--output">
              {outputOmitted > 0 ? `…（前 ${formatCharCount(outputOmitted)} 字符已省略）\n` : ""}
              {outputDisplay || (status === "running" ? "PI Agent 正在准备…" : "尚无输出")}
            </pre>
          )}
        </section>

        {(upstream.length > 0 || downstream.length > 0) && <section className="ai-graph-drawer-section"><div className="ai-graph-drawer-label">依赖关系</div><div className="ai-graph-drawer-deps-chips">{[...upstream, ...downstream].map((id) => <button key={id} className="ai-graph-dep-chip" onClick={() => onSelectNode(id)}>{titleOf(id)}</button>)}</div></section>}

        {nodeRun?.errorText && <section className="ai-graph-drawer-section"><div className="ai-graph-drawer-label">错误</div><div className="ai-graph-drawer-error">{nodeRun.errorText}</div></section>}
      </div>

      {(planStatus === "running" || ["failed", "cancelled", "completed"].includes(planStatus)) && <div className="ai-graph-drawer-footer">{planStatus === "running" ? <Button variant="destructive" size="sm" onClick={onCancel} disabled={actionPending}><Square className="h-3.5 w-3.5" />停止</Button> : <Button variant="outline" size="sm" onClick={onStart} disabled={actionPending}>{planStatus === "completed" ? <RotateCcw className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}重新执行</Button>}</div>}
    </motion.aside>
  );
}

// ── 执行时间线（执行详情主视图） ──

/** 执行过程提示文案：统计口径统一为 timelineRows 行数（工具调用与运行通知
 * 混排）；两者并存时分别列出「N 次调用」与「M 条动态」，避免条数对不上。 */
function executionHint(
  toolCount: number,
  rowCount: number,
  status: ReturnType<typeof normalizeNodeStatus>,
  totals: { input: number; output: number },
): string {
  if (rowCount === 0) {
    return status === "running" ? "等待动态…" : "暂无";
  }
  if (toolCount === 0) {
    return `${rowCount} 条动态`;
  }
  const toolHint = `${toolCount} 次调用 · 入 ${formatCharCount(totals.input)} / 出 ${formatCharCount(totals.output)} 字符`;
  const noticeCount = rowCount - toolCount;
  return noticeCount > 0 ? `${toolHint} · ${noticeCount} 条动态` : toolHint;
}

const TOOL_STATUS_META: Record<ToolCallEntry["status"], { label: string; className: string }> = {
  running: { label: "执行中", className: "ai-graph-tool-status--running" },
  succeeded: { label: "成功", className: "ai-graph-tool-status--succeeded" },
  failed: { label: "失败", className: "ai-graph-tool-status--failed" },
};

/** Agent 输出区渲染上限（取尾部最新内容）。 */
const OUTPUT_DISPLAY_LIMIT = 30_000;
/** 单个工具输入/输出块的渲染上限。 */
const TOOL_BLOCK_DISPLAY_LIMIT = 12_000;
/** activities 空回退的稳定引用（避免行内 `?? []` 每次渲染新建数组）。 */
const EMPTY_ACTIVITIES: AgentActivity[] = [];

/**
 * 超长块截断：
 * - 输入参数保留开头（head）——参数结构通常前置；
 * - 输出结果保留尾部（tail）——报错与结论多在结尾，与 Agent 输出区策略一致。
 */
function truncateBlock(text: string, keep: "head" | "tail"): { text: string; omitted: number } {
  if (text.length <= TOOL_BLOCK_DISPLAY_LIMIT) return { text, omitted: 0 };
  return {
    text: keep === "tail" ? text.slice(-TOOL_BLOCK_DISPLAY_LIMIT) : text.slice(0, TOOL_BLOCK_DISPLAY_LIMIT),
    omitted: text.length - TOOL_BLOCK_DISPLAY_LIMIT,
  };
}

/** 虚拟化执行时间线（工具卡片 + 运行通知混排）：运行中自动跟随滚动，可暂停。 */
function ExecutionTimelineList({ rows, live }: { rows: TimelineRow[]; live: boolean }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [following, setFollowing] = useState(true);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 42,
    overscan: 8,
  });

  useEffect(() => {
    if (following && rows.length > 0) {
      virtualizer.scrollToIndex(rows.length - 1, { align: "end" });
    }
  }, [following, rows.length, virtualizer]);

  if (rows.length === 0) {
    return <p className="ai-graph-drawer-hint ai-graph-tool-empty">{live ? "等待执行动态…" : "尚未记录执行动态。"}</p>;
  }

  return (
    <div className="ai-graph-tool-shell">
      {live && (
        <button type="button" className="ai-graph-follow-toggle" onClick={() => setFollowing((value) => !value)}>
          {following ? <Pause className="h-3 w-3" /> : <ChevronsDown className="h-3 w-3" />}
          {following ? "暂停跟随" : "恢复跟随"}
        </button>
      )}
      <div
        ref={scrollRef}
        className="ai-graph-tool-scroll"
        onScroll={(event) => {
          const target = event.currentTarget;
          if (target.scrollHeight - target.scrollTop - target.clientHeight > 40) setFollowing(false);
        }}
      >
        <div className="ai-graph-tool-virtual" style={{ height: virtualizer.getTotalSize() }}>
          {virtualizer.getVirtualItems().map((item) => {
            const row = rows[item.index];
            return (
              <div
                key={row.kind === "tool" ? row.entry.id : row.notice.id}
                ref={virtualizer.measureElement}
                data-index={item.index}
                className="ai-graph-tool-row"
                style={{ transform: `translateY(${item.start}px)` }}
              >
                {row.kind === "tool" ? <ToolCallCard entry={row.entry} /> : <NoticeRow notice={row.notice} />}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

/** 通知类型 → 图标映射：新增通知类型须在此显式登记，避免隐式 fallback。 */
const NOTICE_ICONS: Record<NodeNotice["kind"], typeof Shrink> = {
  compaction: Shrink,
  retry: RotateCcw,
};

/** 运行通知行：上下文压缩 / 自动重试等节点动态（不可展开）。 */
const NoticeRow = memo(function NoticeRow({ notice }: { notice: NodeNotice }) {
  const NoticeIcon = NOTICE_ICONS[notice.kind];
  return (
    <div className={cn("ai-graph-notice-row", `ai-graph-notice-row--${notice.status}`)}>
      <NoticeIcon className="ai-graph-notice-icon" aria-hidden />
      <span className="ai-graph-notice-title">{notice.title}</span>
      {notice.detail && <span className="ai-graph-notice-detail" title={notice.detail}>{notice.detail}</span>}
    </div>
  );
});

/** 单个工具调用卡片：默认折叠只显示摘要，点击展开格式化后的输入/输出。
 * memo 化：卡片位于虚拟列表 overscan 内，父级高频重渲染（跟随滚动、
 * activities 更新）时仅在 entry 引用变化时才重新渲染。 */
const ToolCallCard = memo(function ToolCallCard({ entry }: { entry: ToolCallEntry }) {
  const [open, setOpen] = useState(false);
  const statusMeta = TOOL_STATUS_META[entry.status];
  const hasDetail = Boolean(entry.inputFormatted || entry.outputFormatted || entry.status === "running");
  // 截断结果按 entry 缓存，避免跟随滚动的高频重渲染重复计算。
  const inputBlock = useMemo(() => truncateBlock(entry.inputFormatted, "head"), [entry.inputFormatted]);
  const outputBlock = useMemo(() => truncateBlock(entry.outputFormatted, "tail"), [entry.outputFormatted]);

  return (
    <div className={cn("ai-graph-tool-card", open && "ai-graph-tool-card--open", `ai-graph-tool-card--${entry.status}`)}>
      <button
        type="button"
        className="ai-graph-tool-card-head"
        onClick={() => hasDetail && setOpen((value) => !value)}
        aria-expanded={open}
      >
        {open ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
        <Wrench className="ai-graph-tool-card-icon" aria-hidden />
        <span className="ai-graph-tool-card-name" title={entry.name}>{entry.name}</span>
        <span className={cn("ai-graph-tool-status", statusMeta.className)}>{statusMeta.label}</span>
        <span className="ai-graph-tool-card-chars" title="输入 / 输出字符数">入 {formatCharCount(entry.inputChars)} · 出 {formatCharCount(entry.outputChars)}</span>
        {entry.durationMs != null && <span className="ai-graph-tool-card-duration">{formatGraphDuration(entry.durationMs)}</span>}
      </button>
      {open && (
        <div className="ai-graph-tool-card-body">
          {entry.inputFormatted ? (
            <div>
              <div className="ai-graph-tool-block-label">输入参数 <span className="ai-graph-drawer-hint">{formatCharCount(entry.inputChars)} 字符</span></div>
              <pre className="ai-graph-tool-pre">{inputBlock.text}</pre>
              {inputBlock.omitted > 0 && <div className="ai-graph-tool-truncated">已截断后 {formatCharCount(inputBlock.omitted)} 字符（保留开头）</div>}
            </div>
          ) : (
            <div className="ai-graph-tool-block-empty">无输入参数</div>
          )}
          {entry.outputFormatted ? (
            <div>
              <div className="ai-graph-tool-block-label">输出结果 <span className="ai-graph-drawer-hint">{formatCharCount(entry.outputChars)} 字符</span></div>
              <pre className="ai-graph-tool-pre">{outputBlock.text}</pre>
              {outputBlock.omitted > 0 && <div className="ai-graph-tool-truncated">已截断前 {formatCharCount(outputBlock.omitted)} 字符（保留尾部）</div>}
            </div>
          ) : (
            <div className="ai-graph-tool-block-empty">{entry.status === "running" ? "执行中，尚无输出…" : "无输出"}</div>
          )}
        </div>
      )}
    </div>
  );
});
