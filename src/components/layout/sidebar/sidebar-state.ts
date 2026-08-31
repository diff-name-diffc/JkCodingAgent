/**
 * 会话侧边栏的纯状态助手：分类分组、展开状态持久化与分类图标映射。
 *
 * 无 React 组件/IPC 依赖（仅图标常量），可独立测试；呈现组件见
 * `SidebarConversationItem` / `SidebarCategoryGroup`，组合入口见 `../sidebar.tsx`。
 */

import * as React from "react";
import {
  Code2,
  Folder,
  GraduationCap,
  Heart,
  Inbox,
  Layers,
  MessageSquarePlus,
} from "lucide-react";
import type { ChatCategory, ChatSession } from "../../../types";

export const UNCATEGORIZED_CATEGORY = "__uncategorized__";
export const UNCATEGORIZED_LABEL = "未分类";
const EXPANDED_STORAGE_KEY = "nezha.chat.v2.expandedCategories";

export interface SidebarCategoryGroup {
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

export function resolveCategoryIcon(iconName: string): React.ElementType {
  return CATEGORY_ICON_MAP[iconName] ?? Folder;
}

export function categoryKey(category: string | null | undefined) {
  return category || UNCATEGORIZED_CATEGORY;
}

/**
 * 按分类把会话分组为侧边栏展示单元。
 *
 * 已知分类始终展示（即使无会话，也需显示 + 按钮以便新建）；
 * 未分类分组仅在有会话时展示（它没有对应实体分类，无法在其下新建）。
 */
export function groupSessionsByCategory(
  sessions: ChatSession[],
  categories: ChatCategory[],
): SidebarCategoryGroup[] {
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

  return groups.filter((group) =>
    group.id !== UNCATEGORIZED_CATEGORY
      ? knownCategoryIds.has(group.id)
      : group.sessions.length > 0,
  );
}

export function loadExpandedCategories(): Set<string> | null {
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

export function saveExpandedCategories(ids: Set<string>) {
  try {
    localStorage.setItem(EXPANDED_STORAGE_KEY, JSON.stringify([...ids]));
  } catch {
    // localStorage 不可用时不影响会话展示。
  }
}
