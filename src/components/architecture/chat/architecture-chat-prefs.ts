/**
 * 架构助手聊天面板的本地偏好（面板内持久化，不回写全局设置）。
 *
 * 键名沿用 `jkcodingagent.*.v1` 惯例；与画布 IndexedDB 持久化键
 * （`jkcodingagent.architecture.v1`）分离。面板宽度由
 * `useDockedBrowserPanel` 用独立宽度键持久化。
 */

import { load, save } from "../../../utils";

export const ARCH_CHAT_PREFS_KEY = "jkcodingagent.architecture.chat.v1";
export const ARCH_CHAT_WIDTH_KEY = "jkcodingagent.architecture.chat.width.v1";

export interface ArchitectureChatPrefs {
  /** 折叠成窄条。 */
  collapsed: boolean;
  /** 面板选定的视觉模型库条目 id；null = 跟随设置中视觉用途绑定。 */
  modelLibraryId: string | null;
  /** 发消息时附带画布截图（视觉感知）。 */
  attachScreenshot: boolean;
  /** 发消息时附带结构化画布快照（文本感知）。 */
  attachSnapshot: boolean;
  /** 当前架构会话 id（懒创建后回填）。 */
  sessionId: string | null;
}

const DEFAULT_PREFS: ArchitectureChatPrefs = {
  collapsed: false,
  modelLibraryId: null,
  attachScreenshot: true,
  attachSnapshot: true,
  sessionId: null,
};

export function loadArchChatPrefs(): ArchitectureChatPrefs {
  const raw = load<Partial<ArchitectureChatPrefs>>(ARCH_CHAT_PREFS_KEY, {});
  // load 只做 JSON.parse、不校验字段类型：逐字段防御，非法类型回落默认值。
  return {
    collapsed: typeof raw.collapsed === "boolean" ? raw.collapsed : DEFAULT_PREFS.collapsed,
    modelLibraryId:
      typeof raw.modelLibraryId === "string" ? raw.modelLibraryId : DEFAULT_PREFS.modelLibraryId,
    attachScreenshot:
      typeof raw.attachScreenshot === "boolean"
        ? raw.attachScreenshot
        : DEFAULT_PREFS.attachScreenshot,
    attachSnapshot:
      typeof raw.attachSnapshot === "boolean" ? raw.attachSnapshot : DEFAULT_PREFS.attachSnapshot,
    sessionId: typeof raw.sessionId === "string" ? raw.sessionId : DEFAULT_PREFS.sessionId,
  };
}

export function saveArchChatPrefs(prefs: ArchitectureChatPrefs): void {
  save(ARCH_CHAT_PREFS_KEY, prefs);
}
