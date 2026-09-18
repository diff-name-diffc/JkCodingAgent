/**
 * 聊天分类图标的共享映射：分类记录只存 lucide 图标名字符串，呈现层
 * （侧边栏分类组、聊天头部分类徽标、分类选择空态）经此解析为组件。
 *
 * 纯常量 + 纯函数，无 IPC / 状态依赖，可独立测试。
 */

import type { ElementType } from "react";
import {
  Code2,
  Folder,
  GraduationCap,
  Heart,
  Inbox,
  Layers,
  MessageSquarePlus,
} from "lucide-react";

const CATEGORY_ICON_MAP: Record<string, ElementType> = {
  MessageSquare: MessageSquarePlus,
  Heart,
  Briefcase: Folder,
  Code2,
  GraduationCap,
  Folder,
  Inbox,
  Layers,
};

/** 未知图标名回退 Folder（与建分类缺省一致，绝不渲染空）。 */
export function resolveCategoryIcon(iconName: string): ElementType {
  return CATEGORY_ICON_MAP[iconName] ?? Folder;
}
