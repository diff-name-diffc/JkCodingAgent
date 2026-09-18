import type { ChatCategory, ChatSession } from "../../types";

/**
 * 解析当前聊天会话所属的分类记录（聊天头部徽标 / 分类化空态共用）。
 *
 * 纯函数：会话无分类、或分类记录已被删除（session.category 指向不存在的
 * id）时返回 null——呈现层据此回退为「不显示徽标」而不是造一个假分类。
 * 非空 category 但找不到记录的场景与侧边栏「未分类」分组口径一致
 * （sidebar-state.groupSessionsByCategory）。
 */
export function resolveActiveChatCategory(
  sessions: ChatSession[],
  categories: ChatCategory[],
  sessionId: string | null,
): ChatCategory | null {
  if (!sessionId) return null;
  const session = sessions.find((item) => item.id === sessionId);
  if (!session?.category) return null;
  return categories.find((category) => category.id === session.category) ?? null;
}
