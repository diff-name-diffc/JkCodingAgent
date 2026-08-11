import * as React from "react";
import {
  ChevronDown,
  ChevronRight,
  Code2,
  Folder,
  GraduationCap,
  Heart,
  Inbox,
  Layers,
  LoaderCircle,
  MoreHorizontal,
  MessageSquarePlus,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  Search,
  Settings,
  Trash2,
} from "lucide-react";
import type { ChatCategory, ChatSession } from "../../types";
import { formatRelativeTime } from "../../utils";
import { useChatCategorySessionsQuery } from "../../hooks/use-chat-queries";
import { useUIStore } from "../../stores/ui-store";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { ScrollArea } from "../ui/scroll-area";
import { Separator } from "../ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { ChatNewCategoryDialog, type ChatCategoryCreateConfig } from "../ChatNewCategoryDialog";
import { ChatCategoryContextMenu } from "../ChatCategoryContextMenu";
import { useDispatcherSessionRunningSet } from "../../hooks/useDispatcherSessionRunningSet";

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

const UNCATEGORIZED_CATEGORY = "__uncategorized__";
const UNCATEGORIZED_LABEL = "未分类";
const EXPANDED_STORAGE_KEY = "nezha.chat.v2.expandedCategories";

interface SidebarCategoryGroup {
  id: string;
  label: string;
  color: string;
  icon: string;
  total: number;
  sessions: ChatSession[];
}

const CATEGORY_ICON_MAP: Record<string, React.ElementType> = {
  MessageSquare: MessageSquarePlus,
  Heart,
  Briefcase: Folder,
  Code2,
  GraduationCap,
  Folder,
  Inbox,
  Layers,
};

function resolveCategoryIcon(iconName: string): React.ElementType {
  return CATEGORY_ICON_MAP[iconName] ?? Folder;
}

function categoryKey(category: string | null | undefined) {
  return category || UNCATEGORIZED_CATEGORY;
}

function loadExpandedCategories(): Set<string> | null {
  try {
    const raw = localStorage.getItem(EXPANDED_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return null;
    return new Set(parsed.filter((item): item is string => typeof item === "string"));
  } catch {
    return null;
  }
}

function saveExpandedCategories(ids: Set<string>) {
  try {
    localStorage.setItem(EXPANDED_STORAGE_KEY, JSON.stringify([...ids]));
  } catch {
    // localStorage 不可用时不影响会话展示。
  }
}

/**
 * Collapsible conversation sidebar for the refactored Chat surface.
 *
 * Category pages are loaded lazily on expansion and advanced independently
 * when that category's bottom sentinel enters the viewport.
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

  const groupedCategories = React.useMemo(() => {
    const sessionsByCategory = new Map<string, ChatSession[]>();
    for (const session of sessions) {
      const key = categoryKey(session.category);
      const list = sessionsByCategory.get(key) ?? [];
      list.push(session);
      sessionsByCategory.set(key, list);
    }

    const sortedCategories = [...categories].sort((a, b) => a.sortOrder - b.sortOrder);
    const knownCategoryIds = new Set(sortedCategories.map((category) => category.id));
    const groups: SidebarCategoryGroup[] = sortedCategories.map((category) => ({
      id: category.id,
      label: category.name,
      color: category.color,
      icon: category.icon,
      total: Math.max(category.sessionCount, sessionsByCategory.get(category.id)?.length ?? 0),
      sessions: sessionsByCategory.get(category.id) ?? [],
    }));

    const uncategorizedSessions = sessions.filter(
      (session) => !session.category || !knownCategoryIds.has(session.category),
    );
    if (uncategorizedSessions.length > 0) {
      groups.push({
        id: UNCATEGORIZED_CATEGORY,
        label: UNCATEGORIZED_LABEL,
        color: "var(--text-muted)",
        icon: "Inbox",
        total: uncategorizedSessions.length,
        sessions: uncategorizedSessions,
      });
    }

    // 已知分类始终展示（即使无会话，也需显示 + 按钮以便新建）；
    // 未分类分组仅在有会话时展示（它没有对应实体分类，无法在其下新建）。
    return groups.filter((group) =>
      group.id !== UNCATEGORIZED_CATEGORY
        ? knownCategoryIds.has(group.id)
        : group.sessions.length > 0,
    );
  }, [categories, sessions]);
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
      <div className="ai-sidebar-search-wrap px-3 pb-2">
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
        <nav className="ai-session-nav px-2 py-2">
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
                  <ConversationItem
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
                  <CategoryGroup
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

function ConversationItem({
  session,
  isRunning,
  active,
  categoryLabel,
  searchQuery,
  categories,
  onMoveSessionToCategory,
  onDeleteSession,
  onSelect,
}: {
  session: ChatSession;
  isRunning: boolean;
  active: boolean;
  categoryLabel?: string;
  searchQuery?: string;
  categories?: ChatCategory[];
  onMoveSessionToCategory?: (sessionId: string, categoryId: string) => void;
  onDeleteSession?: (sessionId: string) => void;
  onSelect: () => void;
}) {
  const trimmedTitle = session.title.trim() || "新对话";
  return (
    <li
      className={cn(
        "ai-session-item group flex items-center gap-2 rounded-md transition-colors",
        searchQuery && "is-search-result",
        active
          ? "ai-session-item-active bg-sidebar-accent text-sidebar-accent-foreground"
          : "text-sidebar-foreground/90 hover:bg-sidebar-hover",
      )}
    >
      <button
        onClick={onSelect}
        aria-current={active ? "true" : undefined}
        className="flex min-w-0 flex-1 items-center gap-2 bg-transparent px-2.5 py-2.5 text-left text-[15px]"
      >
        {isRunning ? (
          <LoaderCircle className="h-3.5 w-3.5 shrink-0 animate-spin text-primary" />
        ) : (
          <span
            className={cn(
              "h-1.5 w-1.5 shrink-0 rounded-full",
              active ? "bg-primary" : "bg-transparent",
            )}
          />
        )}
        <span className="min-w-0 flex-1 truncate">
          <HighlightedSessionTitle title={trimmedTitle} query={searchQuery} />
        </span>
        <span className="shrink-0 text-xs text-muted-foreground/80">
          {formatRelativeTime(session.updatedAt)}
        </span>
        {categoryLabel && (
          <span className="ai-session-category-tag shrink-0 truncate">{categoryLabel}</span>
        )}
      </button>
      {((categories && onMoveSessionToCategory && categories.length > 0) || onDeleteSession) && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label={`打开「${trimmedTitle}」会话操作`}
              className="ai-session-menu-trigger"
            >
              <MoreHorizontal className="h-3.5 w-3.5" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent className="ai-context-menu" align="end">
            {categories && onMoveSessionToCategory && categories.length > 0 && (
              <>
                <DropdownMenuLabel>移动到分类</DropdownMenuLabel>
                <DropdownMenuSeparator className="ai-context-menu-separator" />
                {categories.map((category) => (
                  <DropdownMenuItem
                    key={category.id}
                    className="ai-context-menu-item"
                    disabled={session.category === category.id}
                    onSelect={() => onMoveSessionToCategory(session.id, category.id)}
                  >
                    {category.name}
                  </DropdownMenuItem>
                ))}
              </>
            )}
            {onDeleteSession && (
              <>
                {categories && onMoveSessionToCategory && categories.length > 0 && (
                  <DropdownMenuSeparator className="ai-context-menu-separator" />
                )}
                <DropdownMenuItem
                  className="ai-context-menu-item ai-context-menu-item-danger"
                  onSelect={() => onDeleteSession(session.id)}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                  删除会话
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </li>
  );
}

function HighlightedSessionTitle({ title, query }: { title: string; query?: string }) {
  const needle = query?.trim();
  if (!needle) return title;

  const parts: React.ReactNode[] = [];
  const normalizedTitle = title.toLocaleLowerCase();
  const normalizedNeedle = needle.toLocaleLowerCase();
  let cursor = 0;
  let matchIndex = normalizedTitle.indexOf(normalizedNeedle);

  while (matchIndex >= 0) {
    if (matchIndex > cursor) {
      parts.push(title.slice(cursor, matchIndex));
    }
    const end = matchIndex + needle.length;
    parts.push(
      <mark key={`${matchIndex}-${end}`} className="ai-session-search-match">
        {title.slice(matchIndex, end)}
      </mark>,
    );
    cursor = end;
    matchIndex = normalizedTitle.indexOf(normalizedNeedle, cursor);
  }

  if (cursor === 0) return title;
  if (cursor < title.length) parts.push(title.slice(cursor));
  return parts;
}

function CategoryGroup({
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
}: {
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
}) {
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
              <ConversationItem
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
