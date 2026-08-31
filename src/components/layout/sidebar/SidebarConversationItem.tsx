/**
 * 会话列表行：标题高亮、运行态、分类标签与「移动到分类/删除」操作菜单。
 *
 * 纯呈现组件（无网络/状态机），搜索与分类视图共用。
 */

import * as React from "react";
import { LoaderCircle, MoreHorizontal, Trash2 } from "lucide-react";
import type { ChatCategory, ChatSession } from "../../../types";
import { formatRelativeTime } from "../../../utils";
import { cn } from "../../../lib/cn";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../../ui/dropdown-menu";

export interface SidebarConversationItemProps {
  session: ChatSession;
  isRunning: boolean;
  active: boolean;
  categoryLabel?: string;
  searchQuery?: string;
  categories?: ChatCategory[];
  onMoveSessionToCategory?: (sessionId: string, categoryId: string) => void;
  onDeleteSession?: (sessionId: string) => void;
  onSelect: () => void;
}

export const SidebarConversationItem = React.memo(function SidebarConversationItem({
  session,
  isRunning,
  active,
  categoryLabel,
  searchQuery,
  categories,
  onMoveSessionToCategory,
  onDeleteSession,
  onSelect,
}: SidebarConversationItemProps) {
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
});

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
