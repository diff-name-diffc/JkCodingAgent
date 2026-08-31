import * as React from "react";
import { PanelLeftClose, PanelLeftOpen, Plus, Search, Settings } from "lucide-react";
import type { ChatCategory, ChatSession } from "../../types";
import { useUIStore } from "../../stores/ui-store";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { ScrollArea } from "../ui/scroll-area";
import { Separator } from "../ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";
import { ChatNewCategoryDialog, type ChatCategoryCreateConfig } from "../ChatNewCategoryDialog";
import { useDispatcherSessionRunningSet } from "../../hooks/useDispatcherSessionRunningSet";
import {
  groupSessionsByCategory,
  loadExpandedCategories,
  saveExpandedCategories,
  UNCATEGORIZED_CATEGORY,
} from "./sidebar/sidebar-state";
import { SidebarConversationItem } from "./sidebar/SidebarConversationItem";
import { SidebarCategoryGroup } from "./sidebar/SidebarCategoryGroup";

export interface SidebarProps {
  sessions: ChatSession[];
  categories?: ChatCategory[];
  activeSessionId: string | null;
  onActiveSessionChange: (id: string) => void;
  /** 在指定分类下新建会话。categoryId 为该分类的 id。 */
  onNewSessionInCategory?: (categoryId: string) => void;
  onDeleteSession?: (sessionId: string) => void;
  searchValue: string;
  onSearchChange: (value: string) => void;
  onOpenSettings: () => void;
  onCreateCategory?: (name: string, config?: ChatCategoryCreateConfig) => void;
  onRenameCategory?: (categoryId: string, name: string) => void;
  onDeleteCategory?: (categoryId: string) => void;
  onMoveSessionToCategory?: (sessionId: string, categoryId: string) => void;
  /** Footer slot (usage, settings). */
  footer?: React.ReactNode;
  loading?: boolean;
  error?: string;
  searchActive?: boolean;
}

/**
 * Collapsible conversation sidebar for the refactored Chat surface.
 *
 * 入口只负责组合：分类分组与展开持久化在 `sidebar/sidebar-state`，
 * 行呈现在 `SidebarConversationItem`，分类懒加载分页在 `SidebarCategoryGroup`。
 * Collapses to a 60px icon rail; the wide state shows search + a scrollable
 * conversation list with a clear selected-state.
 */
export function Sidebar({
  sessions,
  categories = [],
  activeSessionId,
  onActiveSessionChange,
  onNewSessionInCategory,
  onDeleteSession,
  searchValue,
  onSearchChange,
  onOpenSettings,
  onCreateCategory,
  onRenameCategory,
  onDeleteCategory,
  onMoveSessionToCategory,
  footer,
  loading,
  error,
  searchActive = false,
}: SidebarProps) {
  const collapsed = useUIStore((s) => s.sidebarCollapsed);
  const toggleSidebar = useUIStore((s) => s.toggleSidebar);
  const persistedExpandedCategoriesRef = React.useRef(loadExpandedCategories());
  const [expandedCategories, setExpandedCategories] = React.useState<Set<string>>(
    () => persistedExpandedCategoriesRef.current ?? new Set(),
  );
  const [categoryDialog, setCategoryDialog] = React.useState<
    { mode: "create"; category: null } | { mode: "rename"; category: ChatCategory } | null
  >(null);

  const groupedCategories = React.useMemo(
    () => groupSessionsByCategory(sessions, categories),
    [categories, sessions],
  );
  const visibleRunningSessionIds = useDispatcherSessionRunningSet(
    React.useMemo(() => sessions.map((session) => session.id), [sessions]),
  );

  React.useEffect(() => {
    if (
      persistedExpandedCategoriesRef.current !== null ||
      searchActive ||
      groupedCategories.length === 0
    ) {
      return;
    }

    // 仅在首次使用、没有持久化偏好时展开当前（或首个）分类。
    // 后续的会话刷新不能覆盖用户手动折叠的选择。
    persistedExpandedCategoriesRef.current = new Set();
    setExpandedCategories((current) => {
      const activeGroup = groupedCategories.find((group) =>
        group.sessions.some((session) => session.id === activeSessionId),
      );
      const targetId = activeGroup?.id ?? groupedCategories[0]?.id;
      if (!targetId || current.has(targetId)) return current;
      const next = new Set(current).add(targetId);
      saveExpandedCategories(next);
      return next;
    });
  }, [activeSessionId, groupedCategories, searchActive]);

  const toggleCategory = React.useCallback((categoryId: string) => {
    setExpandedCategories((current) => {
      const next = new Set(current);
      if (next.has(categoryId)) {
        next.delete(categoryId);
      } else {
        next.add(categoryId);
      }
      saveExpandedCategories(next);
      return next;
    });
  }, []);

  const handleCategorySubmit = React.useCallback(
    (name: string, config?: ChatCategoryCreateConfig) => {
      if (categoryDialog?.mode === "rename") {
        onRenameCategory?.(categoryDialog.category.id, name);
      } else {
        onCreateCategory?.(name, config);
      }
      setCategoryDialog(null);
    },
    [categoryDialog, onCreateCategory, onRenameCategory],
  );

  if (collapsed) {
    return <CollapsedRail onExpand={toggleSidebar} onOpenSettings={onOpenSettings} />;
  }

  return (
    <div className="ai-sidebar-panel flex h-full w-full flex-col">
      {/* Header: collapse + new category（新建会话改到分类行内，可指定分类） */}
      <div className="ai-sidebar-command-row flex items-center gap-2 px-3 py-3">
        <Button variant="ghost" size="icon-sm" aria-label="收起侧边栏" onClick={toggleSidebar}>
          <PanelLeftClose className="h-4 w-4" />
        </Button>
        <div className="flex-1" />
        {onCreateCategory && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                className="ai-sidebar-new-category"
                aria-label="新建分类"
                onClick={() => setCategoryDialog({ mode: "create", category: null })}
              >
                <Plus className="h-4 w-4" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right">新建分类</TooltipContent>
          </Tooltip>
        )}
      </div>

      {/* Search */}
      <div className="px-3 pb-2">
        <div className="ai-sidebar-search relative">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={searchValue}
            onChange={(e) => onSearchChange(e.target.value)}
            placeholder="搜索会话"
            aria-label="搜索会话"
            className="h-9 pl-8 text-sm"
          />
        </div>
      </div>

      <Separator />

      {/* Conversation list */}
      <ScrollArea className="min-h-0 flex-1">
        <nav className="px-2 py-2">
          {loading && (
            <ul className="space-y-1">
              {Array.from({ length: 6 }).map((_, i) => (
                <li key={i} className="h-9 animate-pulse rounded-md bg-secondary" />
              ))}
            </ul>
          )}
          {!loading && error && (
            <p className="ai-session-search-error px-2 py-8 text-center text-xs" title={error}>
              搜索失败，请重试
            </p>
          )}
          {!loading && !error && sessions.length === 0 && (
            <p className="px-2 py-8 text-center text-xs text-muted-foreground">
              {searchValue ? "没有匹配的会话" : "暂无会话"}
            </p>
          )}
          {searchActive ? (
            <section className="ai-session-search-results" aria-label="会话搜索结果">
              <div className="ai-session-search-results-header">
                <Search className="h-3.5 w-3.5" />
                <span>搜索结果</span>
                <span className="ai-session-search-results-count">{sessions.length}</span>
              </div>
              <ul className="space-y-1">
                {sessions.map((session) => (
                  <SidebarConversationItem
                    key={session.id}
                    session={session}
                    isRunning={visibleRunningSessionIds.has(session.id)}
                    active={session.id === activeSessionId}
                    searchQuery={searchValue}
                    categoryLabel={
                      categories.find((category) => category.id === session.category)?.name ??
                      (session.category ? session.category : undefined)
                    }
                    onSelect={() => onActiveSessionChange(session.id)}
                    categories={categories}
                    onMoveSessionToCategory={onMoveSessionToCategory}
                    onDeleteSession={onDeleteSession}
                  />
                ))}
              </ul>
            </section>
          ) : (
            <div className="ai-category-list space-y-2">
              {groupedCategories.map((group) => {
                const expanded = expandedCategories.has(group.id);
                return (
                  <SidebarCategoryGroup
                    key={group.id}
                    category={categories.find((category) => category.id === group.id) ?? null}
                    label={group.label}
                    color={group.color}
                    icon={group.icon}
                    total={group.total}
                    initialSessions={group.sessions}
                    expanded={expanded}
                    activeSessionId={activeSessionId}
                    onToggle={() => toggleCategory(group.id)}
                    onSelectSession={onActiveSessionChange}
                    categories={categories}
                    onNewSession={
                      group.id !== UNCATEGORIZED_CATEGORY && onNewSessionInCategory
                        ? () => onNewSessionInCategory(group.id)
                        : undefined
                    }
                    onRenameCategory={
                      onRenameCategory
                        ? (category) => setCategoryDialog({ mode: "rename", category })
                        : undefined
                    }
                    onDeleteCategory={onDeleteCategory}
                    onMoveSessionToCategory={onMoveSessionToCategory}
                    onDeleteSession={onDeleteSession}
                  />
                );
              })}
            </div>
          )}
        </nav>
      </ScrollArea>

      {footer && (
        <>
          <Separator />
          <div className="ai-sidebar-footer flex items-center gap-1 px-3 py-2">{footer}</div>
        </>
      )}

      <ChatNewCategoryDialog
        open={Boolean(categoryDialog)}
        initialName={categoryDialog?.category?.name ?? ""}
        title={categoryDialog?.mode === "rename" ? "重命名分类" : "新建分类"}
        confirmLabel={categoryDialog?.mode === "rename" ? "保存" : "创建"}
        showAgentConfig={categoryDialog?.mode === "create"}
        onSubmit={handleCategorySubmit}
        onClose={() => setCategoryDialog(null)}
      />
    </div>
  );
}

function CollapsedRail({
  onExpand,
  onOpenSettings,
}: {
  onExpand: () => void;
  onOpenSettings: () => void;
}) {
  return (
    <div className="ai-sidebar-panel ai-sidebar-rail flex h-full w-full flex-col items-center gap-2 py-3">
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon" aria-label="展开侧边栏" onClick={onExpand}>
            <PanelLeftOpen className="h-4 w-4" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="right">展开侧边栏</TooltipContent>
      </Tooltip>
      <div className="flex-1" />
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon" aria-label="设置" onClick={onOpenSettings}>
            <Settings className="h-4 w-4" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="right">设置</TooltipContent>
      </Tooltip>
    </div>
  );
}
