import { useCallback, useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Check,
  ChevronDown,
  Circle,
  Plus,
  RefreshCw,
  Trash2,
  Zap,
} from "lucide-react";
import type {
  AhaContextConfig,
  AhaSharedModels,
  AgentContext,
  DispatcherModelConfig,
} from "../../../types";

const DEFAULT_SUMMARY_MODEL = "deepseek-v4-flash";
const DEFAULT_IMAGE_MODEL_URL = "https://dashscope.aliyuncs.com/api/v1";
const DEFAULT_IMAGE_MODEL = "qwen-image-2.0-pro";
const DEFAULT_ASR_MODEL = "fun-asr-realtime";

type SharedModelKind = "vision" | "image" | "imageEdit" | "asr" | "tts" | "embedding";
type ContextModelKind = "chat" | "summary";
export type Feedback = { status: "success" | "error"; message: string };

export type SectionOptions = {
  urlPlaceholder?: string;
  modelPlaceholder?: string;
  hideModelFetch?: boolean;
};

const SHARED_FIELD_MAP: Record<SharedModelKind, keyof AhaSharedModels> = {
  vision: "visionModelConfigs",
  image: "imageModelConfigs",
  imageEdit: "imageEditModelConfigs",
  asr: "asrModelConfigs",
  tts: "ttsModelConfigs",
  embedding: "embeddingModelConfigs",
};

function cloneModel(model?: Partial<DispatcherModelConfig> | null): DispatcherModelConfig {
  return {
    url: model?.url?.trim() ?? "",
    apiKey: model?.apiKey?.trim() ?? "",
    model: model?.model?.trim() ?? "",
    active: model?.active ?? true,
    systemPrompt: model?.systemPrompt?.trim() ?? "",
  };
}

function normalizeProviders(
  providers: Array<Partial<DispatcherModelConfig> | null | undefined>,
): DispatcherModelConfig[] {
  const normalized = providers.filter(Boolean).map((p) => cloneModel(p));
  if (normalized.length === 0) return [];
  const activeIndex = normalized.findIndex((p) => p.active);
  return normalized.map((p, i) => ({
    ...p,
    active: activeIndex >= 0 ? i === activeIndex : false,
  }));
}

function emptyProvider(): DispatcherModelConfig {
  return { url: "", apiKey: "", model: "", active: false, systemPrompt: "" };
}

function activeProvider(providers: DispatcherModelConfig[]): DispatcherModelConfig {
  return providers.find((p) => p.active) ?? emptyProvider();
}

export function withoutModelSystemPrompts(
  providers: DispatcherModelConfig[],
): DispatcherModelConfig[] {
  return providers.map((provider) => ({ ...provider, systemPrompt: "" }));
}

export function withoutChatModelSystemPrompts(config: AhaContextConfig): AhaContextConfig {
  return {
    ...config,
    chatModelConfigs: withoutModelSystemPrompts(config.chatModelConfigs),
  };
}

export function activeImageModel(shared: AhaSharedModels): DispatcherModelConfig {
  return activeProvider(shared.imageModelConfigs);
}

type RegistryInput = {
  shared: AhaSharedModels;
  setShared: Dispatch<SetStateAction<AhaSharedModels>>;
  project: AhaContextConfig;
  setProject: Dispatch<SetStateAction<AhaContextConfig>>;
  chat: AhaContextConfig;
  setChat: Dispatch<SetStateAction<AhaContextConfig>>;
};

/**
 * 集中管理 Aha 面板内所有 Provider 编辑器的 UI 状态与远程交互：
 * - showKey：是否明文展示 API Key（由面板顶部按钮统一切换）
 * - expanded / testing / fetching / feedback / modelLists / fetchError：以 `key` 索引，
 *   同一时刻全局只允许一个 Provider 处于测试或拉取模型列表状态
 * - renderSharedTab / renderContextModelsTab：渲染对应分类的 Provider 区块
 *
 * 业务状态（shared / project / chat）仍由 AhaAgentPanel 持有并注入，
 * 保存时由面板统一提交 aha_save_settings_v2。
 */
export function useAhaProviderRegistry({
  shared,
  setShared,
  project,
  setProject,
  chat,
  setChat,
}: RegistryInput) {
  const [showKey, setShowKey] = useState(false);
  const [expanded, setExpanded] = useState<Record<string, number>>({});
  const [testing, setTesting] = useState<{ key: string; index: number } | null>(null);
  const [feedback, setFeedback] = useState<Record<string, Partial<Record<number, Feedback>>>>(
    {},
  );
  const [modelLists, setModelLists] = useState<
    Record<string, Partial<Record<number, string[]>>>
  >({});
  const [fetching, setFetching] = useState<{ key: string; index: number } | null>(null);
  const [fetchError, setFetchError] = useState<
    Record<string, Partial<Record<number, string>>>
  >({});

  function updateSharedProviders(
    kind: SharedModelKind,
    updater: (providers: DispatcherModelConfig[]) => DispatcherModelConfig[],
  ) {
    const field = SHARED_FIELD_MAP[kind];
    setShared((prev) => ({
      ...prev,
      [field]: normalizeProviders(updater(prev[field])),
    }));
  }

  function updateContextProviders(
    context: AgentContext,
    kind: ContextModelKind,
    updater: (providers: DispatcherModelConfig[]) => DispatcherModelConfig[],
  ) {
    const setter = context === "project" ? setProject : setChat;
    const field = kind === "chat" ? "chatModelConfigs" : "summaryModelConfigs";
    setter((prev) => ({
      ...prev,
      [field]: normalizeProviders(updater(prev[field])),
    }));
  }

  function addProvider(key: string, providers: DispatcherModelConfig[]) {
    const nextIndex = providers.length;
    setExpanded((prev) => ({ ...prev, [key]: nextIndex }));
    return [
      ...providers,
      {
        url: "",
        apiKey: "",
        model: "",
        active: !providers.some((p) => p.active),
        systemPrompt: "",
      },
    ] as DispatcherModelConfig[];
  }

  function removeProvider(key: string, providers: DispatcherModelConfig[], index: number) {
    setExpanded((prev) => ({
      ...prev,
      [key]: Math.max(0, Math.min(prev[key] ?? 0, providers.length - 2)),
    }));
    return providers.filter((_, i) => i !== index);
  }

  async function testModel(key: string, index: number, config: DispatcherModelConfig) {
    if (testing) return;
    setTesting({ key, index });
    setFeedback((prev) => ({ ...prev, [key]: { ...prev[key], [index]: undefined } }));
    try {
      const kind = key
        .replace(/-[0-9]+$/, "")
        .replace(/^shared-/, "")
        .replace(/^ctx-[^-]+-/, "");
      const message = await invoke<string>("dispatcher_test_model", { kind, config });
      setFeedback((prev) => ({
        ...prev,
        [key]: { ...prev[key], [index]: { status: "success", message } },
      }));
    } catch (error) {
      setFeedback((prev) => ({
        ...prev,
        [key]: { ...prev[key], [index]: { status: "error", message: String(error) } },
      }));
    } finally {
      setTesting(null);
    }
  }

  async function fetchModels(key: string, index: number, config: DispatcherModelConfig) {
    setFetching({ key, index });
    setFetchError((prev) => ({ ...prev, [key]: { ...prev[key], [index]: "" } }));
    try {
      const models = await invoke<string[]>("dispatcher_fetch_models", {
        apiBase: config.url,
        apiKey: config.apiKey,
      });
      setModelLists((prev) => ({ ...prev, [key]: { ...prev[key], [index]: models } }));
    } catch (error) {
      setModelLists((prev) => ({ ...prev, [key]: { ...prev[key], [index]: [] } }));
      setFetchError((prev) => ({ ...prev, [key]: { ...prev[key], [index]: String(error) } }));
    } finally {
      setFetching(null);
    }
  }

  function sharedSection(
    kind: SharedModelKind,
    title: string,
    description: string,
    options: SectionOptions = {},
  ): ReactNode {
    const providers = shared[SHARED_FIELD_MAP[kind]];
    const key = `shared-${kind}`;
    return (
      <ModelProviderSection
        title={title}
        description={description}
        providers={providers}
        expandedIndex={expanded[key] ?? 0}
        showKey={showKey}
        onExpandedChange={(index) => setExpanded((prev) => ({ ...prev, [key]: index }))}
        onChange={(index, patch) =>
          updateSharedProviders(kind, (ps) =>
            ps.map((p, i) => (i === index ? { ...p, ...patch } : p)),
          )
        }
        onAdd={() => updateSharedProviders(kind, (ps) => addProvider(key, ps))}
        onRemove={(index) => updateSharedProviders(kind, (ps) => removeProvider(key, ps, index))}
        onActivate={(index) =>
          updateSharedProviders(kind, (ps) =>
            ps.map((p, i) => ({ ...p, active: i === index ? !p.active : false })),
          )
        }
        onTest={(index) => testModel(key, index, providers[index])}
        onFetchModels={(index) => fetchModels(key, index, providers[index])}
        testing={testing?.key === key ? testing.index : null}
        fetching={fetching?.key === key ? fetching.index : null}
        feedback={feedback[key] ?? {}}
        modelLists={modelLists[key] ?? {}}
        fetchError={fetchError[key] ?? {}}
        {...options}
      />
    );
  }

  function contextSection(
    context: AgentContext,
    kind: ContextModelKind,
    title: string,
    description: string,
    options: SectionOptions = {},
  ): ReactNode {
    const state = context === "project" ? project : chat;
    const providers = state[kind === "chat" ? "chatModelConfigs" : "summaryModelConfigs"];
    const key = `ctx-${context}-${kind}`;
    return (
      <ModelProviderSection
        title={title}
        description={description}
        providers={providers}
        expandedIndex={expanded[key] ?? 0}
        showKey={showKey}
        onExpandedChange={(index) => setExpanded((prev) => ({ ...prev, [key]: index }))}
        onChange={(index, patch) =>
          updateContextProviders(context, kind, (ps) =>
            ps.map((p, i) => (i === index ? { ...p, ...patch } : p)),
          )
        }
        onAdd={() => updateContextProviders(context, kind, (ps) => addProvider(key, ps))}
        onRemove={(index) =>
          updateContextProviders(context, kind, (ps) => removeProvider(key, ps, index))
        }
        onActivate={(index) =>
          updateContextProviders(context, kind, (ps) =>
            ps.map((p, i) => ({ ...p, active: i === index ? !p.active : false })),
          )
        }
        onTest={(index) => testModel(key, index, providers[index])}
        onFetchModels={(index) => fetchModels(key, index, providers[index])}
        testing={testing?.key === key ? testing.index : null}
        fetching={fetching?.key === key ? fetching.index : null}
        feedback={feedback[key] ?? {}}
        modelLists={modelLists[key] ?? {}}
        fetchError={fetchError[key] ?? {}}
        {...options}
      />
    );
  }

  function renderSharedTab(): ReactNode {
    const activeImage = activeProvider(shared.imageModelConfigs);
    return (
      <>
        {sharedSection("vision", "视觉模型", "用户上传图片时使用的多模态模型。")}
        {sharedSection("image", "图片生成模型", "generate_image 工具使用的图片生成模型。", {
          urlPlaceholder: DEFAULT_IMAGE_MODEL_URL,
          modelPlaceholder: DEFAULT_IMAGE_MODEL,
        })}
        {sharedSection(
          "imageEdit",
          "图片编辑模型",
          "edit_image 工具使用的图片编辑模型。",
          {
            urlPlaceholder: DEFAULT_IMAGE_MODEL_URL,
            modelPlaceholder: activeImage.model || DEFAULT_IMAGE_MODEL,
          },
        )}
        {sharedSection(
          "asr",
          "ASR 模型",
          "实时语音识别配置。URL 为 WebSocket 地址。",
          {
            urlPlaceholder: "wss://dashscope.aliyuncs.com/api-ws/v1/inference",
            modelPlaceholder: DEFAULT_ASR_MODEL,
            hideModelFetch: true,
          },
        )}
        {sharedSection("tts", "TTS 模型", "预留的文本转语音模型配置。")}
        {sharedSection("embedding", "文本向量模型", "预留的向量模型配置。")}
      </>
    );
  }

  function renderContextModelsTab(context: AgentContext): ReactNode {
    const label = context === "project" ? "项目" : "聊天";
    return (
      <>
        {contextSection(context, "chat", `${label}主模型`, `${label}对话和工具调用的主模型。`)}
        {contextSection(
          context,
          "summary",
          `${label}摘要模型`,
          `用于工具结果、子任务输出和会话标题摘要；留空时使用默认模型。`,
          { modelPlaceholder: DEFAULT_SUMMARY_MODEL },
        )}
      </>
    );
  }

  return {
    showKey,
    setShowKey,
    renderSharedTab,
    renderContextModelsTab,
  };
}

type ModelProviderSectionProps = {
  title: string;
  description: string;
  providers: DispatcherModelConfig[];
  expandedIndex: number;
  showKey: boolean;
  onExpandedChange: (index: number) => void;
  onChange: (index: number, patch: Partial<DispatcherModelConfig>) => void;
  onAdd: () => void;
  onRemove: (index: number) => void;
  onActivate: (index: number) => void;
  onTest: (index: number) => void;
  onFetchModels: (index: number) => void;
  testing: number | null;
  fetching: number | null;
  feedback: Partial<Record<number, Feedback>>;
  modelLists: Partial<Record<number, string[]>>;
  fetchError: Partial<Record<number, string>>;
} & SectionOptions;

function ModelProviderSection({
  title,
  description,
  providers,
  expandedIndex,
  showKey,
  onExpandedChange,
  onChange,
  onAdd,
  onRemove,
  onActivate,
  onTest,
  onFetchModels,
  testing,
  fetching,
  feedback,
  modelLists,
  fetchError,
  urlPlaceholder = "https://api.example.com/v1",
  modelPlaceholder = "model-name",
  hideModelFetch = false,
}: ModelProviderSectionProps) {
  return (
    <section className="ai-aha-section">
      <div className="ai-aha-section-header">
        <div>
          <div className="ai-aha-section-title">{title}</div>
          <div className="ai-aha-section-description">{description}</div>
        </div>
        <button type="button" className="ai-aha-ghost-button" onClick={onAdd}>
          <Plus size={14} />
          添加 Provider
        </button>
      </div>
      {providers.length === 0 ? (
        <div className="ai-aha-empty-provider">
          还没有 Provider。添加一个后即可配置 URL、Key 和模型名称。
        </div>
      ) : (
        providers.map((provider, index) => (
          <ProviderEditor
            key={index}
            index={index}
            provider={provider}
            expanded={expandedIndex === index}
            showKey={showKey}
            onExpandedChange={() => onExpandedChange(index)}
            onChange={(patch) => onChange(index, patch)}
            onRemove={() => onRemove(index)}
            onActivate={() => onActivate(index)}
            onTest={() => onTest(index)}
            onFetchModels={() => onFetchModels(index)}
            testing={testing === index}
            fetching={fetching === index}
            feedback={feedback[index] ?? null}
            modelList={modelLists[index] ?? []}
            fetchError={fetchError[index]}
            urlPlaceholder={urlPlaceholder}
            modelPlaceholder={modelPlaceholder}
            hideModelFetch={hideModelFetch}
          />
        ))
      )}
    </section>
  );
}

type ProviderEditorProps = {
  index: number;
  provider: DispatcherModelConfig;
  expanded: boolean;
  showKey: boolean;
  onExpandedChange: () => void;
  onChange: (patch: Partial<DispatcherModelConfig>) => void;
  onRemove: () => void;
  onActivate: () => void;
  onTest: () => void;
  onFetchModels: () => void;
  testing: boolean;
  fetching: boolean;
  feedback: Feedback | null;
  modelList: string[];
  fetchError?: string;
  urlPlaceholder: string;
  modelPlaceholder: string;
  hideModelFetch: boolean;
};

function ProviderEditor({
  index,
  provider,
  expanded,
  showKey,
  onExpandedChange,
  onChange,
  onRemove,
  onActivate,
  onTest,
  onFetchModels,
  testing,
  fetching,
  feedback,
  modelList,
  fetchError,
  urlPlaceholder,
  modelPlaceholder,
  hideModelFetch,
}: ProviderEditorProps) {
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const filteredModels = provider.model
    ? modelList.filter((item) => item.toLowerCase().includes(provider.model.toLowerCase()))
    : modelList;
  const canFetch = Boolean(provider.url && provider.apiKey);

  useEffect(() => {
    if (modelList.length > 0) {
      setDropdownOpen(true);
    }
  }, [modelList.length]);

  const handleClickOutside = useCallback((event: MouseEvent) => {
    if (dropdownRef.current && !dropdownRef.current.contains(event.target as Node)) {
      setDropdownOpen(false);
    }
  }, []);

  useEffect(() => {
    if (dropdownOpen) {
      document.addEventListener("mousedown", handleClickOutside);
      return () => document.removeEventListener("mousedown", handleClickOutside);
    }
  }, [dropdownOpen, handleClickOutside]);

  return (
    <div className={provider.active ? "ai-aha-provider is-active" : "ai-aha-provider"}>
      <div className="ai-aha-provider-header">
        <button type="button" className="ai-aha-provider-title-button" onClick={onExpandedChange}>
          <ChevronDown
            size={14}
            className="ai-aha-collapsible-chevron"
            style={{ transform: expanded ? "rotate(0deg)" : "rotate(-90deg)" }}
          />
          <span className="ai-aha-provider-title-wrap">
            <span className="ai-aha-provider-title">Provider {index + 1}</span>
            <span className="ai-aha-provider-summary">
              {provider.model || provider.url || "未配置模型"}
            </span>
          </span>
        </button>
        <div className="ai-aha-provider-actions">
          <button
            type="button"
            className={provider.active ? "ai-aha-active-badge" : "ai-aha-inactive-badge"}
            onClick={onActivate}
            aria-pressed={provider.active}
            title={provider.active ? "点击后取消激活" : "点击后激活当前 Provider"}
          >
            {provider.active ? <Check size={12} /> : <Circle size={12} />}
            {provider.active ? "已激活" : "激活"}
          </button>
          <button
            type="button"
            className="ai-aha-inline-button is-danger"
            onClick={onRemove}
            title="删除当前 Provider"
          >
            <Trash2 size={12} />
            删除
          </button>
        </div>
      </div>
      {expanded && (
        <>
          <div className="ai-aha-grid">
            <label className="ai-aha-field">
              <span className="ai-aha-field-label">URL</span>
              <input
                className="ai-settings-input"
                value={provider.url}
                onChange={(e) => onChange({ url: e.target.value })}
                placeholder={urlPlaceholder}
                spellCheck={false}
              />
            </label>
            <label className="ai-aha-field">
              <span className="ai-aha-field-label">API Key</span>
              <input
                className="ai-settings-input"
                type={showKey ? "text" : "password"}
                value={provider.apiKey}
                onChange={(e) => onChange({ apiKey: e.target.value })}
                placeholder="sk-..."
                spellCheck={false}
              />
            </label>
            <div className="ai-aha-field">
              <div className="ai-aha-field-row">
                <span className="ai-aha-field-label">模型名称</span>
                {!hideModelFetch && (
                  <button
                    type="button"
                    className="ai-aha-inline-button"
                    onClick={onFetchModels}
                    disabled={!canFetch || fetching}
                  >
                    <RefreshCw size={11} className={fetching ? "spin" : undefined} />
                    {fetching ? "获取中..." : "获取模型"}
                  </button>
                )}
              </div>
              <div className="ai-aha-dropdown-wrap" ref={dropdownRef}>
                <input
                  className="ai-settings-input"
                  style={{ paddingRight: modelList.length > 0 ? 32 : 10 }}
                  value={provider.model}
                  onChange={(e) => {
                    onChange({ model: e.target.value });
                    if (modelList.length > 0) setDropdownOpen(true);
                  }}
                  onFocus={() => {
                    if (modelList.length > 0) setDropdownOpen(true);
                  }}
                  placeholder={modelPlaceholder}
                  spellCheck={false}
                />
                {modelList.length > 0 && (
                  <button
                    type="button"
                    onClick={() => setDropdownOpen((o) => !o)}
                    className="ai-aha-dropdown-toggle"
                    aria-label="展开模型列表"
                  >
                    <ChevronDown
                      size={14}
                      style={{
                        transform: dropdownOpen ? "rotate(180deg)" : "none",
                        transition: "transform 0.15s",
                      }}
                    />
                  </button>
                )}
                {dropdownOpen && modelList.length > 0 && (
                  <div className="ai-aha-dropdown">
                    {filteredModels.length === 0 ? (
                      <div className="ai-aha-dropdown-empty">没有匹配的模型</div>
                    ) : (
                      filteredModels.map((item) => (
                        <button
                          key={item}
                          type="button"
                          className={
                            item === provider.model
                              ? "ai-aha-dropdown-item is-current"
                              : "ai-aha-dropdown-item"
                          }
                          onClick={() => {
                            onChange({ model: item });
                            setDropdownOpen(false);
                          }}
                        >
                          {item}
                        </button>
                      ))
                    )}
                  </div>
                )}
              </div>
              {fetchError && (
                <span className="ai-aha-hint is-danger">获取失败：{fetchError}</span>
              )}
            </div>
          </div>
          <div className="ai-aha-action-row">
            <button
              type="button"
              className="ai-aha-ghost-button"
              onClick={onTest}
              disabled={testing}
            >
              <Zap size={14} />
              {testing ? "测试中..." : "测试当前 Provider"}
            </button>
            {feedback && (
              <span
                className={
                  feedback.status === "success"
                    ? "ai-aha-feedback is-success"
                    : "ai-aha-feedback is-error"
                }
                aria-live="polite"
              >
                {feedback.message}
              </span>
            )}
          </div>
        </>
      )}
    </div>
  );
}
