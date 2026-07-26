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
