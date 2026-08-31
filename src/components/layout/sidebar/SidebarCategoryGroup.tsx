/**
 * 分类分组：折叠头、懒加载会话分页（展开时拉取、底部哨兵进入视口翻页）
 * 与分类右键菜单。
 *
 * 数据获取与分页状态机集中于此；行呈现复用 `SidebarConversationItem`。
 */

import * as React from "react";
import { ChevronDown, ChevronRight, Plus } from "lucide-react";
import type { ChatCategory, ChatSession } from "../../../types";
import { useChatCategorySessionsQuery } from "../../../hooks/use-chat-queries";
import { useDispatcherSessionRunningSet } from "../../../hooks/useDispatcherSessionRunningSet";
import { Tooltip, TooltipContent, TooltipTrigger } from "../../ui/tooltip";
import { ChatCategoryContextMenu } from "../../ChatCategoryContextMenu";
import { resolveCategoryIcon } from "./sidebar-state";
import { SidebarConversationItem } from "./SidebarConversationItem";

export interface SidebarCategoryGroupProps {
  category: ChatCategory | null;
  label: string;
  color: string;
  icon: string;
  total: number;
  initialSessions: ChatSession[];
  expanded: boolean;
  activeSessionId: string | null;
  onToggle: () => void;
  onSelectSession: (id: string) => void;
  categories: ChatCategory[];
  onNewSession?: () => void;
  onRenameCategory?: (category: ChatCategory) => void;
  onDeleteCategory?: (categoryId: string) => void;
  onMoveSessionToCategory?: (sessionId: string, categoryId: string) => void;
  onDeleteSession?: (sessionId: string) => void;
}

export function SidebarCategoryGroup({
  category,
  label,
  color,
  icon,
  total,
  initialSessions,
  expanded,
  activeSessionId,
  onToggle,
  onSelectSession,
  categories,
  onNewSession,
  onRenameCategory,
  onDeleteCategory,
  onMoveSessionToCategory,
  onDeleteSession,
}: SidebarCategoryGroupProps) {
  const categorySessionsQuery = useChatCategorySessionsQuery(
    category?.id ?? "",
    expanded && Boolean(category),
  );
  const sessions = React.useMemo(() => {
    const uniqueSessions = new Map<string, ChatSession>();
    for (const session of initialSessions) {
      uniqueSessions.set(session.id, session);
    }
    for (const page of categorySessionsQuery.data?.pages ?? []) {
      for (const session of page.items) {
        uniqueSessions.set(session.id, session);
      }
    }
    return [...uniqueSessions.values()];
  }, [categorySessionsQuery.data?.pages, initialSessions]);
  const displayedTotal = categorySessionsQuery.data?.pages[0]?.total ?? total;
  const runningSessionIds = useDispatcherSessionRunningSet(
    React.useMemo(() => sessions.map((session) => session.id), [sessions]),
  );
  const loadMoreRef = React.useRef<HTMLLIElement | null>(null);
  const { fetchNextPage, hasNextPage, isFetchingNextPage, isFetchNextPageError } =
    categorySessionsQuery;

  React.useEffect(() => {
    const target = loadMoreRef.current;
    if (!expanded || !target || !hasNextPage || isFetchingNextPage || isFetchNextPageError) {
      return;
    }
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry?.isIntersecting) {
          void fetchNextPage();
        }
      },
      { rootMargin: "0px 0px 80px 0px" },
    );
    observer.observe(target);
    return () => observer.disconnect();
  }, [expanded, fetchNextPage, hasNextPage, isFetchNextPageError, isFetchingNextPage]);

  const Icon = resolveCategoryIcon(icon);
  const header = (
    <div
      className="ai-category-header w-full items-center gap-1 rounded-md px-1.5 py-1"
      data-expanded={expanded || undefined}
      style={{ "--category-color": color } as React.CSSProperties}
    >
      <button
        type="button"
        className="ai-category-toggle flex min-w-0 flex-1 items-center gap-2 rounded-md px-1 py-1 text-left"
        onClick={onToggle}
        aria-expanded={expanded}
      >
        {expanded ? (
          <ChevronDown className="h-4 w-4 shrink-0" aria-hidden="true" />
        ) : (
          <ChevronRight className="h-4 w-4 shrink-0" aria-hidden="true" />
        )}
        <Icon className="h-3.5 w-3.5 shrink-0" style={{ color }} />
        <span className="min-w-0 flex-1 truncate">{label}</span>
      </button>
      <span className="ai-category-count">{displayedTotal}</span>
      {onNewSession && (
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              aria-label={`在「${label}」中新建会话`}
              className="ai-category-new-session flex h-6 w-6 shrink-0 items-center justify-center rounded-md transition-colors"
              onClick={(e) => {
                e.stopPropagation();
                onNewSession();
              }}
            >
              <Plus className="h-3.5 w-3.5" />
            </button>
          </TooltipTrigger>
          <TooltipContent side="right">新建会话</TooltipContent>
        </Tooltip>
      )}
    </div>
  );

  return (
    <section className="ai-category-group">
      {category && (onRenameCategory || onDeleteCategory) ? (
        <ChatCategoryContextMenu
          category={category}
          onRename={() => onRenameCategory?.(category)}
          onDelete={() => onDeleteCategory?.(category.id)}
        >
          {header}
        </ChatCategoryContextMenu>
      ) : (
        header
      )}

      {expanded && (
        <ul className="ai-category-sessions mt-1 space-y-0.5">
          {categorySessionsQuery.isLoading && sessions.length === 0 ? (
            <li className="px-7 py-2 text-xs text-muted-foreground">正在加载会话…</li>
          ) : categorySessionsQuery.isError && sessions.length === 0 ? (
            <li className="px-7 py-2 text-xs text-destructive">
              <button type="button" onClick={() => void categorySessionsQuery.refetch()}>
                加载失败，点击重试
              </button>
            </li>
          ) : sessions.length === 0 ? (
            <li className="px-7 py-2 text-xs text-muted-foreground">暂无会话</li>
          ) : (
            sessions.map((session) => (
              <SidebarConversationItem
                key={session.id}
                session={session}
                isRunning={runningSessionIds.has(session.id)}
                active={session.id === activeSessionId}
                onSelect={() => onSelectSession(session.id)}
                categories={categories}
                onMoveSessionToCategory={onMoveSessionToCategory}
                onDeleteSession={onDeleteSession}
              />
            ))
          )}
          {hasNextPage && (
            <li ref={loadMoreRef} className="px-7 py-2 text-xs text-muted-foreground">
              {isFetchNextPageError ? (
                <button type="button" onClick={() => void fetchNextPage()}>
                  加载下一页失败，点击重试
                </button>
              ) : isFetchingNextPage ? (
                "正在加载更多…"
              ) : (
                "继续滚动加载更多"
              )}
            </li>
          )}
        </ul>
      )}
    </section>
  );
}
