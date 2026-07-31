import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AhaSettingsV2, ChatCategoryAgentConfig } from "../../types";
import { toast } from "./toast";
import { withoutChatModelSystemPrompts } from "./providers/provider-registry";
import { hasAnyPurposeConfigs, seedModelLibrary } from "./providers/model-library";

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
如果设置中启用了聊天 MCP 工具，可按工具说明调用这些动态发现的外部工具。
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

/** 集中持有 Aha 设置并提供失焦/变更自动保存管线（替代旧的全局手动保存按钮）。 */
export function useAhaSettingsStore(): AhaSettingsStore {
  const [settings, setSettings] = useState<AhaSettingsV2 | null>(null);
  const [chatCategoryConfigs, setChatCategoryConfigs] = useState<ChatCategoryAgentConfig[]>([]);
  const [activeChatCategoryId, setActiveChatCategoryId] = useState<string | null>(null);
  const [globalEnabledIds, setGlobalEnabledIds] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [pending, setPending] = useState(false);
  const [saveError, setSaveError] = useState<SaveError>(null);

  // 保存是异步的，通过 ref 读取最新状态，避免闭包捕获过期值。
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const categoryConfigsRef = useRef(chatCategoryConfigs);
  categoryConfigsRef.current = chatCategoryConfigs;
  const enabledIdsRef = useRef(globalEnabledIds);
  enabledIdsRef.current = globalEnabledIds;
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const fieldIdRef = useRef<string | undefined>(undefined);
  const savingRef = useRef(false);
  // 保存进行期间又来了新变更时置位，完成后立即补一次保存，避免丢失。
  const dirtyDuringSaveRef = useRef(false);

  const saveNow = useCallback(async (): Promise<boolean> => {
    const current = settingsRef.current;
    if (!current) return true;
    if (savingRef.current) {
      dirtyDuringSaveRef.current = true;
      return true;
    }
    savingRef.current = true;
    setSaving(true);
    setSaveError(null);
    try {
      const payload: AhaSettingsV2 = {
        ...current,
        chat: withoutChatModelSystemPrompts(current.chat),
      };
      const [result, savedCategoryConfigs] = await Promise.all([
        invoke<AhaSettingsV2>("aha_save_settings_v2", { settings: payload }),
        invoke<ChatCategoryAgentConfig[]>("aha_save_chat_category_agent_configs", {
          configs: categoryConfigsRef.current,
        }),
        invoke("sub_agent_set_global_enabled", { subAgentIds: enabledIdsRef.current }),
      ]);
      setSettings({ ...result, chat: withoutChatModelSystemPrompts(result.chat) });
      setChatCategoryConfigs(savedCategoryConfigs);
      setActiveChatCategoryId((currentId) =>
        currentId && savedCategoryConfigs.some((c) => c.categoryId === currentId)
          ? currentId
          : (savedCategoryConfigs[0]?.categoryId ?? null),
      );
      toast.success("已保存");
      return true;
    } catch (error) {
      const message = String(error);
      setSaveError({ fieldId: fieldIdRef.current, message });
      toast.error(`保存失败：${message}`);
      return false;
    } finally {
      savingRef.current = false;
      setSaving(false);
      if (dirtyDuringSaveRef.current) {
        dirtyDuringSaveRef.current = false;
        void saveNow();
      }
    }
  }, []);

  const scheduleSave = useCallback(
    (fieldId?: string) => {
      fieldIdRef.current = fieldId ?? fieldIdRef.current;
      setPending(true);
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        setPending(false);
        void saveNow();
      }, AUTOSAVE_DELAY_MS);
    },
    [saveNow],
  );

  useEffect(() => {
    Promise.all([
      invoke<AhaSettingsV2>("aha_get_settings_v2"),
      invoke<ChatCategoryAgentConfig[]>("aha_get_chat_category_agent_configs"),
      invoke<Array<{ id: string }>>("sub_agent_get_global_enabled").then((agents) =>
        agents.map((agent) => agent.id),
      ),
    ])
      .then(([loaded, categoryConfigs, enabledIds]) => {
        // 首次迁移：旧用户已有用途配置但模型库为空时，按分类播种模型库并落盘。
        const needsSeed =
          (loaded.modelLibrary ?? []).length === 0 && hasAnyPurposeConfigs(loaded);
        const settings = needsSeed
          ? { ...loaded, modelLibrary: seedModelLibrary(loaded) }
          : loaded;
        setSettings(settings);
        setChatCategoryConfigs(categoryConfigs);
        setActiveChatCategoryId(categoryConfigs[0]?.categoryId ?? null);
        setGlobalEnabledIds(enabledIds);
        if (needsSeed) scheduleSave();
      })
      .catch((error) => toast.error(`设置加载失败：${String(error)}`))
      .finally(() => setLoading(false));
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, [scheduleSave]);

  const updateSettings = useCallback(
    (updater: (prev: AhaSettingsV2) => AhaSettingsV2, fieldId?: string) => {
      setSettings((prev) => (prev ? updater(prev) : prev));
      setSaveError(null);
      scheduleSave(fieldId);
    },
    [scheduleSave],
  );

  const updateChatCategoryConfigs = useCallback(
    (configs: ChatCategoryAgentConfig[]) => {
      setChatCategoryConfigs(configs);
      scheduleSave();
    },
    [scheduleSave],
  );

  const updateGlobalEnabledIds = useCallback(
    (ids: string[]) => {
      setGlobalEnabledIds(ids);
      scheduleSave();
    },
    [scheduleSave],
  );

  const reloadChatCategoryConfigs = useCallback(async () => {
    const configs = await invoke<ChatCategoryAgentConfig[]>("aha_get_chat_category_agent_configs");
    setChatCategoryConfigs(configs);
    setActiveChatCategoryId((currentId) =>
      currentId && configs.some((c) => c.categoryId === currentId)
        ? currentId
        : (configs[0]?.categoryId ?? null),
    );
  }, []);

  const flush = useCallback(async (): Promise<boolean> => {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    setPending(false);
    if (!settingsRef.current) return true;
    return saveNow();
  }, [saveNow]);

  return useMemo(
    () => ({
      settings,
      loading,
      dirty: pending || saving,
      saving,
      saveError,
      chatCategoryConfigs,
      activeChatCategoryId,
      setActiveChatCategoryId,
      globalEnabledIds,
      updateSettings,
      updateChatCategoryConfigs,
      updateGlobalEnabledIds,
      reloadChatCategoryConfigs,
      flush,
    }),
    [
      settings,
      loading,
      pending,
      saving,
      saveError,
      chatCategoryConfigs,
      activeChatCategoryId,
      globalEnabledIds,
      updateSettings,
      updateChatCategoryConfigs,
      updateGlobalEnabledIds,
      reloadChatCategoryConfigs,
      flush,
    ],
  );
}
