import { memo, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ChevronDown,
  ChevronRight,
  ChevronsDown,
  Pause,
  RotateCcw,
  Shrink,
  Wrench,
} from "lucide-react";
import type { GraphNodeStatus } from "../../types";
import { cn } from "../../lib/cn";
import {
  formatCharCount,
  formatGraphDuration,
  type NodeNotice,
  type TimelineRow,
  type ToolCallEntry,
} from "./graph-utils";

// ── 执行时间线（节点抽屉「执行过程」主视图；自 GraphNodeDrawer 拆出，UI-14） ──

/** 执行过程提示文案：统计口径统一为 timelineRows 行数（工具调用与运行通知
 * 混排）；两者并存时分别列出「N 次调用」与「M 条动态」，避免条数对不上。 */
export function executionHint(
  toolCount: number,
  rowCount: number,
  status: GraphNodeStatus,
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

/** 单个工具输入/输出块的渲染上限。 */
const TOOL_BLOCK_DISPLAY_LIMIT = 12_000;

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
export function ExecutionTimelineList({ rows, live }: { rows: TimelineRow[]; live: boolean }) {
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
