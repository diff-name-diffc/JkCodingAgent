import { createContext, useContext, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AhaSettingsV2, ChatCategoryAgentConfig } from "../../types";
import { toast } from "./toast";
import { withoutChatModelSystemPrompts } from "./providers/provider-registry";

export type SaveError = { fieldId?: string; message: string } | null;

export type AhaSettingsStore = {
  settings: AhaSettingsV2 | null;
  loading: boolean;
  /** 有等待中的 debounce 或正在进行的保存。 */
  dirty: boolean;
  saving: boolean;
  saveError: SaveError;
  chatCategoryConfigs: ChatCategoryAgentConfig[];
  activeChatCategoryId: string | null;
  setActiveChatCategoryId: (id: string | null) => void;
  globalEnabledIds: string[];
  /** 更新业务状态并调度自动保存；fieldId 用于保存失败时在对应字段下方内联报错。 */
  updateSettings: (updater: (prev: AhaSettingsV2) => AhaSettingsV2, fieldId?: string) => void;
  updateChatCategoryConfigs: (configs: ChatCategoryAgentConfig[]) => void;
  updateGlobalEnabledIds: (ids: string[]) => void;
  reloadChatCategoryConfigs: () => Promise<void>;
  /** 立即保存（关闭弹窗前调用）；返回是否成功。 */
  flush: () => Promise<boolean>;
};

const AhaSettingsContext = createContext<AhaSettingsStore | null>(null);

export function useAhaSettings(): AhaSettingsStore {
  const store = useContext(AhaSettingsContext);
  if (!store) throw new Error("useAhaSettings 必须在 AhaSettingsProvider 内使用");
  return store;
}

export const AhaSettingsProvider = AhaSettingsContext.Provider;

const AUTOSAVE_DELAY_MS = 400;

/**
 * 聊天分类系统提示词的默认值，与 Rust 侧 DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT
 * （src-tauri/src/agent/config.rs）保持一致，用于「恢复默认」按钮。
 */
export const DEFAULT_CHAT_CATEGORY_SYSTEM_PROMPT = `# 普通聊天

你是桌面客户端中的普通聊天助手。
当前会话不是项目 Agent 会话，没有项目目录、项目文件系统或子进程能力。
你可以调用 local_zsh 在受限本地目录 .jkcodingagent/local_env/zsh 中执行 macOS zsh 命令；所有产物应留在该目录，工具会维护 audit.json 审计历史。
如果配置了全局 MCP 服务器，其第三方工具会以 mcp__ 前缀动态注入你的工具列表，可按工具说明直接调用。
你可以按需使用浏览器工具打开网页、点击、输入、等待、读取页面可访问性树快照、请求视觉辅助分析和关闭浏览器，用于网页自动化与公开信息检索。
浏览器自动化统一使用 ref：先调用 browser_read_text 获取 Accessibility Tree 快照，再使用快照中的 ref 调用点击、输入或局部读取工具；不要使用 CSS selector。
元素 ref 只在最近一次 browser_read_text 快照中有效。页面导航或内容变化后旧 ref 会失效，收到 ref 失效错误时系统会自动附上新快照，基于新快照重新选择元素即可。
检索问题信息时，优先打开明确网址；没有网址时可打开搜索引擎结果页并读取页面文本，不要伪造检索结果。
可以基于用户直接提供的文本、代码片段、错误信息或图片进行解释、分析、改写和建议。
默认使用简体中文，表达直接、清晰、面向有经验的开发者。

## 子智能体

- 你可以调用 list_sub_agents 查看当前可用的子智能体列表。
- 使用 call_sub_agent(agent_id, task) 调用子智能体处理特定领域的复杂任务。子智能体拥有独立的执行上下文，内部工具调用对你透明，你只会收到最终结果。

## 图片生成与引用

- 你可以调用 generate_image 工具根据文本描述生成图片。建议提供 image_name 参数为图片命名。
- 你可以调用 edit_image 工具对现有图片进行编辑。需要提供图片的本地绝对路径。
- 工具返回结果中会包含该图片的本地绝对路径。
- 如果你想在回答中展示生成的图片，直接使用 Markdown 图片引用语法引用工具返回的原始本地绝对路径即可。
`;

// ── 模块级单例状态 ────────────────────────────────────────────────────────────
// 设置弹窗可随多个 keep-alive 项目页同时挂载、也会随项目关闭被连带卸载，
// 因此状态与保存管线放模块级（模式参照 graph-store.ts）：
// 弹窗卸载不中断 debounce/保存，多个弹窗实例共享同一份状态、编辑互相同步。

let settings: AhaSettingsV2 | null = null;
let chatCategoryConfigs: ChatCategoryAgentConfig[] = [];
let activeChatCategoryId: string | null = null;
let globalEnabledIds: string[] = [];
let loading = true;
let saving = false;
let pending = false;
let saveError: SaveError = null;

/** 本地编辑修订号：保存回写前比对，避免服务端返回值覆盖保存期间的新编辑。 */
let revision = 0;
/** 加载守卫：并发挂载只加载一次；加载失败后复位，允许下次挂载重试。 */
let loadStarted = false;
let saveTimer: ReturnType<typeof setTimeout> | null = null;
let pendingFieldId: string | undefined;
/** 保存进行期间又来了新变更时置位，完成后立即补一次保存，避免丢失。 */
let dirtyDuringSave = false;

type Subscriber = () => void;
const subscribers = new Set<Subscriber>();

/** 对外快照：数据变更时重建，函数引用稳定，供 useSyncExternalStore 比较。 */
let snapshot: AhaSettingsStore = buildSnapshot();

function buildSnapshot(): AhaSettingsStore {
  return {
    settings,
    loading,
    dirty: pending || saving,
    saving,
    saveError,
    chatCategoryConfigs,
    activeChatCategoryId,
    setActiveChatCategoryId: setActiveChatCategoryIdAction,
    globalEnabledIds,
    updateSettings,
    updateChatCategoryConfigs,
    updateGlobalEnabledIds,
    reloadChatCategoryConfigs,
    flush,
  };
}

function notify(): void {
  snapshot = buildSnapshot();
  for (const subscriber of subscribers) subscriber();
}

function ensureLoaded(): void {
  if (loadStarted) return;
  loadStarted = true;
  Promise.all([
    invoke<AhaSettingsV2>("aha_get_settings_v2"),
    invoke<ChatCategoryAgentConfig[]>("aha_get_chat_category_agent_configs"),
    invoke<Array<{ id: string }>>("sub_agent_get_global_enabled").then((agents) =>
      agents.map((agent) => agent.id),
    ),
  ])
    .then(([loaded, categoryConfigs, enabledIds]) => {
      settings = loaded;
      chatCategoryConfigs = categoryConfigs;
      activeChatCategoryId = categoryConfigs[0]?.categoryId ?? null;
      globalEnabledIds = enabledIds;
    })
    .catch((error) => {
      toast.error(`设置加载失败：${String(error)}`);
      loadStarted = false;
    })
    .finally(() => {
      loading = false;
      notify();
    });
}

function subscribe(subscriber: Subscriber): () => void {
  subscribers.add(subscriber);
  ensureLoaded();
  return () => {
    subscribers.delete(subscriber);
  };
}

function getSnapshot(): AhaSettingsStore {
  return snapshot;
}

async function saveNow(): Promise<boolean> {
  const current = settings;
  if (!current) return true;
  if (saving) {
    dirtyDuringSave = true;
    return true;
  }
  const revisionAtStart = revision;
  saving = true;
  saveError = null;
  notify();
  try {
    const payload: AhaSettingsV2 = {
      ...current,
      chat: withoutChatModelSystemPrompts(current.chat),
    };
    const [result, savedCategoryConfigs] = await Promise.all([
      invoke<AhaSettingsV2>("aha_save_settings_v2", { settings: payload }),
      invoke<ChatCategoryAgentConfig[]>("aha_save_chat_category_agent_configs", {
        configs: chatCategoryConfigs,
      }),
      invoke("sub_agent_set_global_enabled", { subAgentIds: globalEnabledIds }),
    ]);
    if (revision === revisionAtStart) {
      // 保存期间无本地编辑，可用服务端返回值安全回填。
      settings = { ...result, chat: withoutChatModelSystemPrompts(result.chat) };
      chatCategoryConfigs = savedCategoryConfigs;
      if (
        !activeChatCategoryId ||
        !savedCategoryConfigs.some((c) => c.categoryId === activeChatCategoryId)
      ) {
        activeChatCategoryId = savedCategoryConfigs[0]?.categoryId ?? null;
      }
      notify();
    } else {
      // 保存期间用户又编辑过：保留本地状态，交由 finally 里的补存落库。
      dirtyDuringSave = true;
    }
    toast.success("已保存");
    return true;
  } catch (error) {
    const message = String(error);
    saveError = { fieldId: pendingFieldId, message };
    toast.error(`保存失败：${message}`);
    return false;
  } finally {
    saving = false;
    notify();
    if (dirtyDuringSave) {
      dirtyDuringSave = false;
      void saveNow();
    }
  }
}

function scheduleSave(fieldId?: string): void {
  pendingFieldId = fieldId ?? pendingFieldId;
  pending = true;
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    pending = false;
    void saveNow();
  }, AUTOSAVE_DELAY_MS);
  notify();
}

function updateSettings(
  updater: (prev: AhaSettingsV2) => AhaSettingsV2,
  fieldId?: string,
): void {
  if (!settings) {
    // 加载未完成/失败时不能静默丢弃编辑：明确告知而不是假装已调度保存。
    toast.error("设置尚未加载完成，请稍后再试");
    return;
  }
  revision += 1;
  settings = updater(settings);
  saveError = null;
  scheduleSave(fieldId);
}

function updateChatCategoryConfigs(configs: ChatCategoryAgentConfig[]): void {
  revision += 1;
  chatCategoryConfigs = configs;
  scheduleSave();
}

function updateGlobalEnabledIds(ids: string[]): void {
  revision += 1;
  globalEnabledIds = ids;
  scheduleSave();
}

function setActiveChatCategoryIdAction(id: string | null): void {
  activeChatCategoryId = id;
  notify();
}

async function reloadChatCategoryConfigs(): Promise<void> {
  const configs = await invoke<ChatCategoryAgentConfig[]>("aha_get_chat_category_agent_configs");
  // 递增 revision：重载结果也是本地新状态，若此时有保存在途，
  // 保存完成后的回写分支会因 revision 变化而跳过，不会覆盖重载结果。
  revision += 1;
  chatCategoryConfigs = configs;
  if (!activeChatCategoryId || !configs.some((c) => c.categoryId === activeChatCategoryId)) {
    activeChatCategoryId = configs[0]?.categoryId ?? null;
  }
  notify();
}

async function flush(): Promise<boolean> {
  if (saveTimer) {
    clearTimeout(saveTimer);
    saveTimer = null;
  }
  pending = false;
  notify();
  if (!settings) return true;
  // 保存仍在进行时只标记补存（saveNow 的 dirtyDuringSave 分支），不丢编辑。
  return saveNow();
}

/**
 * 订阅模块级单例 Aha 设置 store（失焦/变更自动保存管线，替代旧的全局手动保存按钮）。
 * 首个订阅者挂载时惰性加载一次；之后所有弹窗实例共享同一份状态。
 */
export function useAhaSettingsStore(): AhaSettingsStore {
  return useSyncExternalStore(subscribe, getSnapshot);
}
