import * as React from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronDown } from "lucide-react";
import { summarizeToolActivity, type ToolActivityItem } from "../dispatcher-chat/tool-activity";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { usePersistedToggle, useRowUiStateContext } from "./row-ui-state";
import { ToolActivityRow, type ToolActivityActions } from "./tool-activity-row";
import { selectVisibleToolActivities } from "./tool-activity-presentation";

export interface ToolCallListProps extends ToolActivityActions {
  items: ToolActivityItem[];
  className?: string;
  /** 所属消息行的稳定 id；用户的全览选择在消息虚拟化卸载后仍可恢复。 */
  rowId?: string;
}

export const ToolCallList = React.memo(function ToolCallList({
  items,
  className,
  rowId,
  onOpenArtifact,
  onOpenSubAgent,
  onConfigureModel,
}: ToolCallListProps) {
  const [showAll, setShowAll] = usePersistedToggle(
    rowId === undefined ? undefined : `tools:${rowId}`,
    false,
  );
  const rowUiState = useRowUiStateContext();
  const [expandedIds, setExpandedIds] = React.useState<ReadonlySet<string>>(
    () =>
      new Set(items.filter((item) => rowUiState?.get(`card:${item.id}`)).map((item) => item.id)),
  );
  const summary = React.useMemo(() => summarizeToolActivity(items), [items]);
  const compactItems = React.useMemo(
    () => selectVisibleToolActivities(items, false, expandedIds),
    [items, expandedIds],
  );
  const visibleItems = showAll ? items : compactItems;
  const hiddenCount = items.length - compactItems.length;
  const contentId = React.useId();
  const scrollRef = React.useRef<HTMLDivElement>(null);
  const windowed = visibleItems.length > 40;
  const getItemKey = React.useCallback((index: number) => visibleItems[index].id, [visibleItems]);
  const virtualizer = useVirtualizer({
    count: visibleItems.length,
    enabled: windowed,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 38,
    getItemKey,
    overscan: 6,
  });
  const onExpandedChange = React.useCallback(
    (id: string, expanded: boolean) => {
      rowUiState?.set(`card:${id}`, expanded);
      setExpandedIds((current) => {
        if (current.has(id) === expanded) return current;
        const next = new Set(current);
        if (expanded) next.add(id);
        else next.delete(id);
        return next;
      });
    },
    [rowUiState],
  );
  const row = (item: ToolActivityItem) => (
    <ToolActivityRow
      item={item}
      expanded={expandedIds.has(item.id)}
      onExpandedChange={onExpandedChange}
      onOpenArtifact={onOpenArtifact}
      onOpenSubAgent={onOpenSubAgent}
      onConfigureModel={onConfigureModel}
    />
  );

  if (!items.length) return null;

  return (
    <div className={cn("space-y-1", className)}>
      <div className="flex min-h-7 flex-wrap items-center gap-x-3 gap-y-1 px-2 text-[11px] text-muted-foreground">
        <span className="font-medium">
          工具活动 <span className="tabular-nums">{items.length}</span>
        </span>
        {summary.running > 0 && <span className="text-primary">{summary.running} 项进行中</span>}
        {summary.planned > 0 && <span>{summary.planned} 项等待</span>}
        {summary.failed > 0 && <span className="text-destructive">{summary.failed} 项失败</span>}
        {summary.running === 0 && summary.planned === 0 && summary.failed === 0 && (
          <span>已完成</span>
        )}
      </div>
      <div
        id={contentId}
        ref={scrollRef}
        className={cn(windowed && "chat-scroll max-h-96 overflow-y-auto overscroll-contain")}
      >
        {windowed ? (
          <div
            role="list"
            aria-label="工具活动"
            className="relative"
            style={{ height: virtualizer.getTotalSize() }}
          >
            {virtualizer.getVirtualItems().map((virtualRow) => (
              <div
                key={virtualRow.key}
                ref={virtualizer.measureElement}
                data-index={virtualRow.index}
                role="listitem"
                aria-setsize={visibleItems.length}
                aria-posinset={virtualRow.index + 1}
                className="absolute left-0 top-0 w-full"
                style={{ transform: `translateY(${virtualRow.start}px)` }}
              >
                {row(visibleItems[virtualRow.index])}
              </div>
            ))}
          </div>
        ) : (
          <div role="list" aria-label="工具活动">
            {visibleItems.map((item) => (
              <div key={item.id} role="listitem">
                {row(item)}
              </div>
            ))}
          </div>
        )}
      </div>
      {(hiddenCount > 0 || showAll) && (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="ml-6 h-7 gap-1.5 px-2 text-[11px] text-muted-foreground"
          aria-expanded={showAll}
          aria-controls={contentId}
          onClick={() => setShowAll((current) => !current)}
        >
          <ChevronDown
            aria-hidden
            className={cn(
              "h-3 w-3 transition-transform duration-fast motion-reduce:transition-none",
              showAll && "rotate-180",
            )}
          />
          {showAll ? "收起较早活动" : `查看全部 ${items.length} 项活动 · 另有 ${hiddenCount} 项`}
        </Button>
      )}
    </div>
  );
});
