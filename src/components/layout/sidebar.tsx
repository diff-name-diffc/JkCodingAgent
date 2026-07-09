import * as React from "react";
import { motion } from "framer-motion";
import {
  ChevronLeft,
  ChevronRight,
  Code2,
  Folder,
  GraduationCap,
  Heart,
  Inbox,
  Layers,
  MoreHorizontal,
  MessageSquarePlus,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  Search,
  Settings,
} from "lucide-react";
import type { ChatCategory, ChatSession } from "../../types";
import { useUIStore } from "../../stores/ui-store";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { ScrollArea } from "../ui/scroll-area";
import { Separator } from "../ui/separator";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "../ui/tooltip";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import {
  ChatNewCategoryDialog,
  type ChatCategoryCreateConfig,
} from "../ChatNewCategoryDialog";
import { ChatCategoryContextMenu } from "../ChatCategoryContextMenu";

export interface SidebarProps {
  sessions: ChatSession[];
  categories?: ChatCategory[];
  activeSessionId: string | null;
  onActiveSessionChange: (id: string) => void;
  onNewConversation: () => void;
  searchValue: string;
  onSearchChange: (value: string) => void;
  onOpenSettings: () => void;
  onCreateCategory?: (name: string, config?: ChatCategoryCreateConfig) => void;
  onRenameCategory?: (categoryId: string, name: string) => void;
  onDeleteCategory?: (categoryId: string) => void;
  onMoveSessionToCategory?: (sessionId: string, categoryId: string) => void;
  /** Footer slot (theme toggle, usage, user). */
  footer?: React.ReactNode;
  loading?: boolean;
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

function loadExpandedCategories(): Set<string> {
  try {
    const raw = localStorage.getItem(EXPANDED_STORAGE_KEY);
    if (!raw) return new Set();
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((item): item is string => typeof item === "string"));
  } catch {
    return new Set();
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
 * Pure presentational. Data is supplied by the parent (which wires the
 * existing Tauri invokes via TanStack Query — see use-chat-queries.ts).
 * Collapses to a 60px icon rail; the wide state shows search + a scrollable
 * conversation list with a clear selected-state.
 */
export function Sidebar({
  sessions,
  categories = [],
  activeSessionId,
  onActiveSessionChange,
  onNewConversation,
  searchValue,
  onSearchChange,
  onOpenSettings,
  onCreateCategory,
  onRenameCategory,
  onDeleteCategory,
  onMoveSessionToCategory,
  footer,
  loading,
  searchActive = false,
}: SidebarProps) {
  const collapsed = useUIStore((s) => s.sidebarCollapsed);
  const toggleSidebar = useUIStore((s) => s.toggleSidebar);
  const [expandedCategories, setExpandedCategories] =
    React.useState<Set<string>>(() => loadExpandedCategories());
  const [categoryDialog, setCategoryDialog] = React.useState<
    | { mode: "create"; category: null }
    | { mode: "rename"; category: ChatCategory }
    | null
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

    return groups.filter((group) => group.total > 0 || group.sessions.length > 0);
  }, [categories, sessions]);

  React.useEffect(() => {
    if (searchActive || groupedCategories.length === 0) return;
    setExpandedCategories((current) => {
      const next = new Set(current);
      let changed = false;
      const activeGroup = groupedCategories.find((group) =>
        group.sessions.some((session) => session.id === activeSessionId),
      );
      const fallbackGroup = groupedCategories[0];
      const targetId = activeGroup?.id ?? (next.size === 0 ? fallbackGroup?.id : null);
      if (targetId && !next.has(targetId)) {
        next.add(targetId);
        changed = true;
      }
      if (changed) saveExpandedCategories(next);
      return changed ? next : current;
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
    return <CollapsedRail onNewConversation={onNewConversation} onExpand={toggleSidebar} onOpenSettings={onOpenSettings} />;
  }

  return (
    <div className="ai-sidebar-panel flex h-full w-full flex-col">
      {/* Header: collapse + new chat */}
      <div className="ai-sidebar-command-row flex items-center gap-2 px-3 py-3">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="收起侧边栏"
          onClick={toggleSidebar}
        >
          <PanelLeftClose className="h-4 w-4" />
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="ai-new-chat-button flex-1 justify-start gap-2"
          onClick={onNewConversation}
        >
          <MessageSquarePlus className="h-4 w-4" />
          新建对话
        </Button>
        {onCreateCategory && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
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
            className="h-8 pl-8 text-xs"
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
                <li
                  key={i}
                  className="h-9 animate-pulse rounded-md bg-secondary"
                />
              ))}
            </ul>
          )}
          {!loading && sessions.length === 0 && (
            <p className="px-2 py-8 text-center text-xs text-muted-foreground">
              {searchValue ? "没有匹配的会话" : "暂无会话"}
            </p>
          )}
          {searchActive ? (
            <ul className="space-y-0.5">
              {sessions.map((session) => (
                <ConversationItem
                  key={session.id}
                  session={session}
                  active={session.id === activeSessionId}
                  categoryLabel={
                    categories.find((category) => category.id === session.category)?.name ??
                    (session.category ? session.category : undefined)
                  }
                  onSelect={() => onActiveSessionChange(session.id)}
                  categories={categories}
                  onMoveSessionToCategory={onMoveSessionToCategory}
                />
              ))}
            </ul>
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
                    expanded={expanded}
                    sessions={group.sessions}
                    activeSessionId={activeSessionId}
                    onToggle={() => toggleCategory(group.id)}
                    onSelectSession={onActiveSessionChange}
                    categories={categories}
                    onRenameCategory={
                      onRenameCategory
                        ? (category) => setCategoryDialog({ mode: "rename", category })
                        : undefined
                    }
                    onDeleteCategory={onDeleteCategory}
                    onMoveSessionToCategory={onMoveSessionToCategory}
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
  active,
  categoryLabel,
  categories,
  onMoveSessionToCategory,
  onSelect,
}: {
  session: ChatSession;
  active: boolean;
  categoryLabel?: string;
  categories?: ChatCategory[];
  onMoveSessionToCategory?: (sessionId: string, categoryId: string) => void;
  onSelect: () => void;
}) {
  const trimmedTitle = session.title.trim() || "新对话";
  return (
    <li
      className={cn(
        "ai-session-item group flex items-center gap-2 rounded-md transition-colors",
        active
          ? "ai-session-item-active bg-sidebar-accent text-sidebar-accent-foreground"
          : "text-sidebar-foreground/90 hover:bg-sidebar-hover",
      )}
    >
      <button
        onClick={onSelect}
        aria-current={active ? "true" : undefined}
        className="flex min-w-0 flex-1 items-center gap-2 bg-transparent px-2.5 py-2 text-left text-sm"
      >
        <span
          className={cn(
            "h-1.5 w-1.5 shrink-0 rounded-full",
            session.isRunning
              ? "bg-primary animate-pulse"
              : active
                ? "bg-primary"
                : "bg-transparent",
          )}
        />
        <span className="min-w-0 flex-1 truncate">{trimmedTitle}</span>
        {categoryLabel && (
          <span className="ai-session-category-tag shrink-0 truncate">
            {categoryLabel}
          </span>
        )}
      </button>
      {categories && onMoveSessionToCategory && categories.length > 0 && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="移动会话分类"
              className="ai-session-menu-trigger"
            >
              <MoreHorizontal className="h-3.5 w-3.5" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent className="ai-context-menu" align="end">
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
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </li>
  );
}

function CategoryGroup({
  category,
  label,
  color,
  icon,
  total,
  expanded,
  sessions,
  activeSessionId,
  onToggle,
  onSelectSession,
  categories,
  onRenameCategory,
  onDeleteCategory,
  onMoveSessionToCategory,
}: {
  category: ChatCategory | null;
  label: string;
  color: string;
  icon: string;
  total: number;
  expanded: boolean;
  sessions: ChatSession[];
  activeSessionId: string | null;
  onToggle: () => void;
  onSelectSession: (id: string) => void;
  categories: ChatCategory[];
  onRenameCategory?: (category: ChatCategory) => void;
  onDeleteCategory?: (categoryId: string) => void;
  onMoveSessionToCategory?: (sessionId: string, categoryId: string) => void;
}) {
  const Icon = resolveCategoryIcon(icon);
  const header = (
      <button
        type="button"
        className="ai-category-header flex w-full items-center gap-2 rounded-md px-2.5 py-2 text-left"
        onClick={onToggle}
        aria-expanded={expanded}
      >
        <ChevronRight
          className={cn("h-3.5 w-3.5 shrink-0 transition-transform", expanded && "rotate-90")}
        />
        <Icon className="h-3.5 w-3.5 shrink-0" style={{ color }} />
        <span className="min-w-0 flex-1 truncate">{label}</span>
        <span className="ai-category-count">{total}</span>
      </button>
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
          {sessions.length === 0 ? (
            <li className="px-7 py-2 text-xs text-muted-foreground">暂无会话</li>
          ) : (
            sessions.map((session) => (
              <ConversationItem
                key={session.id}
                session={session}
                active={session.id === activeSessionId}
                onSelect={() => onSelectSession(session.id)}
                categories={categories}
                onMoveSessionToCategory={onMoveSessionToCategory}
              />
            ))
          )}
        </ul>
      )}
    </section>
  );
}

function CollapsedRail({
  onNewConversation,
  onExpand,
  onOpenSettings,
}: {
  onNewConversation: () => void;
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
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon" aria-label="新建对话" onClick={onNewConversation}>
            <MessageSquarePlus className="h-4 w-4" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="right">新建对话 ⌘N</TooltipContent>
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

/** Small animated caret used by the sidebar header (kept here for reuse). */
export function SidebarChevron({ open }: { open: boolean }) {
  return (
    <motion.span animate={{ rotate: open ? 0 : -90 }} transition={{ duration: 0.15 }}>
      <ChevronLeft className="h-3.5 w-3.5" />
    </motion.span>
  );
}
