import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Check,
  ChevronDown,
  Database,
  Eye,
  EyeOff,
  ImageIcon,
  MessageCircle,
  Mic,
  Plus,
  RefreshCw,
  Trash2,
  type LucideIcon,
  Zap,
} from "lucide-react";
import type { DispatcherModelConfig, DispatcherSettings } from "../../../types";
import s from "../../../styles";

const DEFAULT_SUMMARY_MODEL = "deepseek-v4-flash";
const DEFAULT_IMAGE_MODEL_URL = "https://dashscope.aliyuncs.com/api/v1";
const DEFAULT_IMAGE_MODEL = "qwen-image-2.0-pro";
const DEFAULT_ASR_MODEL = "fun-asr-realtime";

type AhaTab = "chat" | "vision" | "image" | "voice" | "embedding";
type ModelKind =
  | "chat"
  | "summary"
  | "vision"
  | "image"
  | "imageEdit"
  | "asr"
  | "tts"
  | "embedding";
type Feedback = { status: "success" | "error"; message: string };

type AhaSettingsDraft = {
  chatModelConfigs: DispatcherModelConfig[];
  summaryModelConfigs: DispatcherModelConfig[];
  visionModelConfigs: DispatcherModelConfig[];
  imageModelConfigs: DispatcherModelConfig[];
  imageEditModelConfigs: DispatcherModelConfig[];
  asrModelConfigs: DispatcherModelConfig[];
  ttsModelConfigs: DispatcherModelConfig[];
  embeddingModelConfigs: DispatcherModelConfig[];
  autoApproveDispatch: boolean;
  contextDebug: boolean;
};

const tabs: Array<{ key: AhaTab; label: string; icon: LucideIcon }> = [
  { key: "chat", label: "聊天主模型", icon: MessageCircle },
  { key: "vision", label: "视觉模型", icon: Eye },
  { key: "image", label: "图片模型", icon: ImageIcon },
  { key: "voice", label: "语音模型", icon: Mic },
  { key: "embedding", label: "文本向量模型", icon: Database },
];

function cloneModel(model?: Partial<DispatcherModelConfig> | null): DispatcherModelConfig {
  return {
    url: model?.url?.trim() ?? "",
    apiKey: model?.apiKey?.trim() ?? "",
    model: model?.model?.trim() ?? "",
    active: model?.active ?? true,
  };
}

function normalizeProviders(
  providers: Array<Partial<DispatcherModelConfig> | null | undefined>,
): DispatcherModelConfig[] {
  const normalized = providers
    .filter(Boolean)
    .map((provider) => cloneModel(provider))
    .filter((provider) => provider.url || provider.apiKey || provider.model);
  if (normalized.length === 0) return [];

  const activeIndex = normalized.findIndex((provider) => provider.active);
  return normalized.map((provider, index) => ({
    ...provider,
    active: index === (activeIndex >= 0 ? activeIndex : 0),
  }));
}

function normalizeDraftProviders(
  providers: Array<Partial<DispatcherModelConfig> | null | undefined>,
): DispatcherModelConfig[] {
  const normalized = providers.filter(Boolean).map((provider) => cloneModel(provider));
  if (normalized.length === 0) return [];

  const activeIndex = normalized.findIndex((provider) => provider.active);
  return normalized.map((provider, index) => ({
    ...provider,
    active: index === (activeIndex >= 0 ? activeIndex : 0),
  }));
}

function providersFromSettings(
  providers: DispatcherModelConfig[] | undefined,
  single: DispatcherModelConfig | undefined,
  legacy: DispatcherModelConfig,
): DispatcherModelConfig[] {
  const normalized = normalizeProviders(providers ?? []);
  if (normalized.length > 0) return normalized;

  const singleProvider = normalizeProviders([single ?? legacy]);
  return singleProvider.length > 0 ? singleProvider : [cloneModel(legacy)];
}

function activeProvider(providers: DispatcherModelConfig[]): DispatcherModelConfig {
  return providers.find((provider) => provider.active) ?? providers[0] ?? cloneModel(null);
}

function withFallbacks(
  kind: ModelKind,
  provider: DispatcherModelConfig,
  imageProvider?: DispatcherModelConfig,
): DispatcherModelConfig {
  if (kind === "summary") {
    return { ...provider, model: provider.model || DEFAULT_SUMMARY_MODEL };
  }
  if (kind === "image") {
    return {
      ...provider,
      url: provider.url || DEFAULT_IMAGE_MODEL_URL,
      model: provider.model || DEFAULT_IMAGE_MODEL,
    };
  }
  if (kind === "imageEdit") {
    return {
      url: provider.url || imageProvider?.url || DEFAULT_IMAGE_MODEL_URL,
      apiKey: provider.apiKey || imageProvider?.apiKey || "",
      model: provider.model || imageProvider?.model || DEFAULT_IMAGE_MODEL,
      active: provider.active,
    };
  }
  if (kind === "asr") {
    return { ...provider, model: provider.model || DEFAULT_ASR_MODEL };
  }
  return provider;
}

export function settingsToDraft(settings: DispatcherSettings | null): AhaSettingsDraft {
  const legacyChat = cloneModel({
    url: settings?.apiBase,
    apiKey: settings?.apiKey,
    model: settings?.model,
  });
  const legacySummary = cloneModel({
    url: settings?.apiBase,
    apiKey: settings?.apiKey,
    model: settings?.summaryModel || DEFAULT_SUMMARY_MODEL,
  });
  const legacyVision = cloneModel({
    url: settings?.apiBase,
    apiKey: settings?.apiKey,
    model: settings?.visionModel,
  });
  const legacyImage = cloneModel({
    url: settings?.imageModelUrl || DEFAULT_IMAGE_MODEL_URL,
    apiKey: settings?.imageModelApiKey,
    model: settings?.imageModel || DEFAULT_IMAGE_MODEL,
  });
  const legacyImageEdit = cloneModel({
    url: settings?.imageModelUrl || DEFAULT_IMAGE_MODEL_URL,
    apiKey: settings?.imageModelApiKey,
    model: settings?.imageEditModel || settings?.imageModel || DEFAULT_IMAGE_MODEL,
  });
  const legacyAsr = cloneModel({
    url: settings?.asrWebsocketUrl,
    apiKey: settings?.asrApiKey,
    model: DEFAULT_ASR_MODEL,
  });

  return {
    chatModelConfigs: providersFromSettings(
      settings?.chatModelConfigs,
      settings?.chatModelConfig,
      legacyChat,
    ),
    summaryModelConfigs: providersFromSettings(
      settings?.summaryModelConfigs,
      settings?.summaryModelConfig,
      legacySummary,
    ),
    visionModelConfigs: providersFromSettings(
      settings?.visionModelConfigs,
      settings?.visionModelConfig,
      legacyVision,
    ),
    imageModelConfigs: providersFromSettings(
      settings?.imageModelConfigs,
      settings?.imageModelConfig,
      legacyImage,
    ),
    imageEditModelConfigs: providersFromSettings(
      settings?.imageEditModelConfigs,
      settings?.imageEditModelConfig,
      legacyImageEdit,
    ),
    asrModelConfigs: providersFromSettings(
      settings?.asrModelConfigs,
      settings?.asrModelConfig,
      legacyAsr,
    ),
    ttsModelConfigs: normalizeProviders(settings?.ttsModelConfigs ?? [settings?.ttsModelConfig]),
    embeddingModelConfigs: normalizeProviders(
      settings?.embeddingModelConfigs ?? [settings?.embeddingModelConfig],
    ),
    autoApproveDispatch: settings?.autoApproveDispatch ?? false,
    contextDebug: settings?.contextDebug ?? false,
  };
}

export function draftToSavePayload(draft: AhaSettingsDraft) {
  const chatProviders = normalizeProviders(draft.chatModelConfigs);
  const summaryProviders = normalizeProviders(draft.summaryModelConfigs);
  const visionProviders = normalizeProviders(draft.visionModelConfigs);
  const imageProviders = normalizeProviders(draft.imageModelConfigs).map((provider) =>
    withFallbacks("image", provider),
  );
  const activeImage = activeProvider(imageProviders);
  const imageEditProviders = normalizeProviders(draft.imageEditModelConfigs).map((provider) =>
    withFallbacks("imageEdit", provider, activeImage),
  );
  const asrProviders = normalizeProviders(draft.asrModelConfigs).map((provider) =>
    withFallbacks("asr", provider),
  );
  const ttsProviders = normalizeProviders(draft.ttsModelConfigs);
  const embeddingProviders = normalizeProviders(draft.embeddingModelConfigs);
  const activeChat = activeProvider(chatProviders);
  const activeSummary = withFallbacks("summary", activeProvider(summaryProviders));
  const activeVision = activeProvider(visionProviders);
  const activeImageEdit = withFallbacks(
    "imageEdit",
    activeProvider(imageEditProviders),
    activeImage,
  );
  const activeAsr = withFallbacks("asr", activeProvider(asrProviders));
  const activeTts = activeProvider(ttsProviders);
  const activeEmbedding = activeProvider(embeddingProviders);

  return {
    apiBase: activeChat.url,
    apiKey: activeChat.apiKey,
    model: activeChat.model,
    summaryModel: activeSummary.model,
    visionModel: activeVision.model,
    asrApiKey: activeAsr.apiKey,
    asrWebsocketUrl: activeAsr.url,
    autoApproveDispatch: draft.autoApproveDispatch,
    contextDebug: draft.contextDebug,
    imageModelUrl: activeImage.url || DEFAULT_IMAGE_MODEL_URL,
    imageModelApiKey: activeImage.apiKey,
    imageModel: activeImage.model || DEFAULT_IMAGE_MODEL,
    imageEditModel: activeImageEdit.model || activeImage.model,
    chatModelConfig: activeChat,
    summaryModelConfig: activeSummary,
    visionModelConfig: activeVision,
    imageModelConfig: activeImage,
    imageEditModelConfig: activeImageEdit,
    asrModelConfig: activeAsr,
    ttsModelConfig: activeTts,
    embeddingModelConfig: activeEmbedding,
    chatModelConfigs: chatProviders,
    summaryModelConfigs: summaryProviders.map((provider) => withFallbacks("summary", provider)),
    visionModelConfigs: visionProviders,
    imageModelConfigs: imageProviders,
    imageEditModelConfigs: imageEditProviders,
    asrModelConfigs: asrProviders,
    ttsModelConfigs: ttsProviders,
    embeddingModelConfigs: embeddingProviders,
  };
}

export function AhaAgentPanel() {
  const [activeTab, setActiveTab] = useState<AhaTab>("chat");
  const [draft, setDraft] = useState<AhaSettingsDraft>(() => settingsToDraft(null));
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [showKey, setShowKey] = useState(false);
  const [expanded, setExpanded] = useState<Record<ModelKind, number>>({
    chat: 0,
    summary: 0,
    vision: 0,
    image: 0,
    imageEdit: 0,
    asr: 0,
    tts: 0,
    embedding: 0,
  });
  const [testing, setTesting] = useState<{ kind: ModelKind; index: number } | null>(null);
  const [feedback, setFeedback] = useState<Record<ModelKind, Partial<Record<number, Feedback>>>>({
    chat: {},
    summary: {},
    vision: {},
    image: {},
    imageEdit: {},
    asr: {},
    tts: {},
    embedding: {},
  });
  const [modelLists, setModelLists] = useState<
    Record<ModelKind, Partial<Record<number, string[]>>>
  >({
    chat: {},
    summary: {},
    vision: {},
    image: {},
    imageEdit: {},
    asr: {},
    tts: {},
    embedding: {},
  });
  const [fetching, setFetching] = useState<{ kind: ModelKind; index: number } | null>(null);
  const [fetchError, setFetchError] = useState<Record<ModelKind, Partial<Record<number, string>>>>({
    chat: {},
    summary: {},
    vision: {},
    image: {},
    imageEdit: {},
    asr: {},
    tts: {},
    embedding: {},
  });

  useEffect(() => {
    invoke<DispatcherSettings | null>("dispatcher_get_settings")
      .then((settings) => setDraft(settingsToDraft(settings)))
      .catch(console.error)
      .finally(() => setLoading(false));
  }, []);

  function updateProviders(
    kind: ModelKind,
    updater: (providers: DispatcherModelConfig[]) => DispatcherModelConfig[],
  ) {
    const key = modelKey(kind);
    setDraft((prev) => ({
      ...prev,
      [key]: normalizeDraftProviders(updater(prev[key])),
    }));
  }

  function updateProvider(kind: ModelKind, index: number, patch: Partial<DispatcherModelConfig>) {
    updateProviders(kind, (providers) =>
      providers.map((provider, providerIndex) =>
        providerIndex === index ? { ...provider, ...patch } : provider,
      ),
    );
  }

  function addProvider(kind: ModelKind) {
    const nextIndex = draft[modelKey(kind)].length;
    updateProviders(kind, (providers) => [
      ...providers,
      { url: "", apiKey: "", model: "", active: providers.length === 0 },
    ]);
    setExpanded((prev) => ({ ...prev, [kind]: nextIndex }));
  }

  function removeProvider(kind: ModelKind, index: number) {
    updateProviders(kind, (providers) => {
      const next = providers.filter((_, providerIndex) => providerIndex !== index);
      if (next.length === 0) return [];
      if (!next.some((provider) => provider.active)) {
        next[0] = { ...next[0], active: true };
      }
      return next;
    });
    setExpanded((prev) => ({
      ...prev,
      [kind]: Math.max(0, Math.min(prev[kind], draft[modelKey(kind)].length - 2)),
    }));
  }

  function activateProvider(kind: ModelKind, index: number) {
    updateProviders(kind, (providers) =>
      providers.map((provider, providerIndex) => ({
        ...provider,
        active: providerIndex === index,
      })),
    );
    setExpanded((prev) => ({ ...prev, [kind]: index }));
  }

  async function handleSave() {
    setSaving(true);
    setSaved(false);
    setSaveError(null);
    try {
      const savedSettings = await invoke<DispatcherSettings>("dispatcher_save_settings", {
        settings: draftToSavePayload(draft),
      });
      setDraft(settingsToDraft(savedSettings));
      if (savedSettings.contextDebug !== draft.contextDebug) {
        setSaveError(
          "上下文调试开关尚未被后端接受。若刚修改了 src-tauri 代码，请重启 pnpm tauri dev 后再保存一次。",
        );
        return;
      }
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2000);
    } catch (error) {
      setSaveError(String(error));
    } finally {
      setSaving(false);
    }
  }

  async function testModel(kind: ModelKind, index: number) {
    if (testing) return;
    const config = draft[modelKey(kind)][index];
    setTesting({ kind, index });
    setFeedback((prev) => ({ ...prev, [kind]: { ...prev[kind], [index]: undefined } }));
    try {
      const message = await invoke<string>("dispatcher_test_model", { kind, config });
      setFeedback((prev) => ({
        ...prev,
        [kind]: { ...prev[kind], [index]: { status: "success", message } },
      }));
    } catch (error) {
      setFeedback((prev) => ({
        ...prev,
        [kind]: { ...prev[kind], [index]: { status: "error", message: String(error) } },
      }));
    } finally {
      setTesting(null);
    }
  }

  async function fetchModels(kind: ModelKind, index: number) {
    const config = draft[modelKey(kind)][index];
    setFetching({ kind, index });
    setFetchError((prev) => ({ ...prev, [kind]: { ...prev[kind], [index]: "" } }));
    try {
      const models = await invoke<string[]>("dispatcher_fetch_models", {
        apiBase: config.url,
        apiKey: config.apiKey,
      });
      setModelLists((prev) => ({ ...prev, [kind]: { ...prev[kind], [index]: models } }));
    } catch (error) {
      setModelLists((prev) => ({ ...prev, [kind]: { ...prev[kind], [index]: [] } }));
      setFetchError((prev) => ({ ...prev, [kind]: { ...prev[kind], [index]: String(error) } }));
    } finally {
      setFetching(null);
    }
  }

  function modelSection(
    kind: ModelKind,
    title: string,
    description: string,
    options: SectionOptions = {},
  ) {
    return (
      <ModelProviderSection
        title={title}
        description={description}
        providers={draft[modelKey(kind)]}
        expandedIndex={expanded[kind]}
        showKey={showKey}
        onExpandedChange={(index) => setExpanded((prev) => ({ ...prev, [kind]: index }))}
        onChange={(index, patch) => updateProvider(kind, index, patch)}
        onAdd={() => addProvider(kind)}
        onRemove={(index) => removeProvider(kind, index)}
        onActivate={(index) => activateProvider(kind, index)}
        onTest={(index) => testModel(kind, index)}
        onFetchModels={(index) => fetchModels(kind, index)}
        testing={testing?.kind === kind ? testing.index : null}
        fetching={fetching?.kind === kind ? fetching.index : null}
        feedback={feedback[kind]}
        modelLists={modelLists[kind]}
        fetchError={fetchError[kind]}
        {...options}
      />
    );
  }

  function renderActiveBody() {
    if (activeTab === "chat") {
      return (
        <>
          {modelSection("chat", "主聊天模型", "Aha 对话和工具调用的主模型。")}
          {modelSection(
            "summary",
            "摘要模型",
            "用于工具结果、子任务输出和会话标题摘要；留空时使用默认模型。",
            {
              modelPlaceholder: DEFAULT_SUMMARY_MODEL,
            },
          )}
        </>
      );
    }

    if (activeTab === "vision") {
      return modelSection(
        "vision",
        "视觉模型",
        "用户上传图片时使用的多模态模型。留空时图片请求会提示配置缺失。",
      );
    }

    if (activeTab === "image") {
      return (
        <>
          {modelSection("image", "图片生成模型", "generate_image 工具使用的图片生成模型。", {
            urlPlaceholder: DEFAULT_IMAGE_MODEL_URL,
            modelPlaceholder: DEFAULT_IMAGE_MODEL,
          })}
          {modelSection(
            "imageEdit",
            "图片编辑模型",
            "edit_image 工具使用的图片编辑模型；模型名留空时保存层回退到当前图片生成模型。",
            {
              urlPlaceholder: DEFAULT_IMAGE_MODEL_URL,
              modelPlaceholder:
                activeProvider(draft.imageModelConfigs).model || DEFAULT_IMAGE_MODEL,
            },
          )}
        </>
      );
    }

    if (activeTab === "voice") {
      return (
        <>
          {modelSection(
            "asr",
            "ASR 模型",
            "实时语音识别配置。URL 为 WebSocket 地址；测试不会启动真实录音会话。",
            {
              urlPlaceholder: "wss://dashscope.aliyuncs.com/api-ws/v1/inference",
              modelPlaceholder: DEFAULT_ASR_MODEL,
              hideModelFetch: true,
            },
          )}
          {modelSection(
            "tts",
            "TTS 模型",
            "预留的文本转语音模型配置；本次只保存和测试，不接入运行链路。",
          )}
        </>
      );
    }

    return modelSection(
      "embedding",
      "文本向量模型",
      "预留的向量模型配置；本次只保存和测试，不改变知识库或 Aha 运行链路。",
    );
  }

  return (
    <>
      <div style={s.ahaPanel}>
        <div style={s.ahaTabs} role="tablist" aria-label="Aha 模型配置类型">
          {tabs.map((tab) => {
            const Icon = tab.icon;
            const selected = activeTab === tab.key;
            return (
              <button
                key={tab.key}
                type="button"
                role="tab"
                aria-selected={selected}
                style={{
                  ...s.ahaTab,
                  background: selected ? "var(--bg-hover)" : "transparent",
                  borderColor: selected ? "var(--border-medium)" : "transparent",
                  color: selected ? "var(--text-primary)" : "var(--text-muted)",
                }}
                onClick={() => setActiveTab(tab.key)}
              >
                <Icon size={14} />
                {tab.label}
              </button>
            );
          })}
        </div>
        <div style={s.ahaBody}>
          {loading ? (
            <div style={{ color: "var(--text-hint)", fontSize: 13 }}>加载中...</div>
          ) : (
            <div style={s.ahaContent}>
              <div style={{ ...s.ahaActionRow, justifyContent: "flex-end" }}>
                <button
                  type="button"
                  style={s.ahaGhostButton}
                  onClick={() => setShowKey((value) => !value)}
                >
                  {showKey ? <EyeOff size={14} /> : <Eye size={14} />}
                  {showKey ? "隐藏 Key" : "显示 Key"}
                </button>
              </div>
              {renderActiveBody()}
              <BehaviorSection
                autoApprove={draft.autoApproveDispatch}
                contextDebug={draft.contextDebug}
                onAutoApproveChange={(value) =>
                  setDraft((prev) => ({ ...prev, autoApproveDispatch: value }))
                }
                onContextDebugChange={(value) =>
                  setDraft((prev) => ({ ...prev, contextDebug: value }))
                }
              />
            </div>
          )}
        </div>
      </div>
      <div style={s.settingsFooter}>
        {saveError && (
          <span style={{ ...s.ahaFeedback, color: "var(--danger)", marginRight: "auto" }}>
            {saveError}
          </span>
        )}
        {saved && (
          <span
            style={{
              ...s.ahaFeedback,
              display: "inline-flex",
              alignItems: "center",
              gap: 4,
              color: "var(--success)",
              marginRight: saveError ? 12 : "auto",
            }}
          >
            <Check size={12} /> 已保存
          </span>
        )}
        <button
          type="button"
          style={{ ...s.modalSaveBtn, opacity: saving ? 0.5 : 1 }}
          onClick={handleSave}
          disabled={saving}
        >
          {saving ? "保存中..." : "保存"}
        </button>
      </div>
    </>
  );
}

function modelKey(
  kind: ModelKind,
): keyof Pick<
  AhaSettingsDraft,
  | "chatModelConfigs"
  | "summaryModelConfigs"
  | "visionModelConfigs"
  | "imageModelConfigs"
  | "imageEditModelConfigs"
  | "asrModelConfigs"
  | "ttsModelConfigs"
  | "embeddingModelConfigs"
> {
  const keys = {
    chat: "chatModelConfigs",
    summary: "summaryModelConfigs",
    vision: "visionModelConfigs",
    image: "imageModelConfigs",
    imageEdit: "imageEditModelConfigs",
    asr: "asrModelConfigs",
    tts: "ttsModelConfigs",
    embedding: "embeddingModelConfigs",
  } as const;
  return keys[kind];
}

type SectionOptions = {
  urlPlaceholder?: string;
  modelPlaceholder?: string;
  hideModelFetch?: boolean;
};

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
}: {
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
} & SectionOptions) {
  return (
    <section style={s.ahaSection}>
      <div style={s.ahaSectionHeader}>
        <div>
          <div style={s.ahaSectionTitle}>{title}</div>
          <div style={s.ahaSectionDescription}>{description}</div>
        </div>
        <button type="button" style={s.ahaGhostButton} onClick={onAdd}>
          <Plus size={14} />
          添加 Provider
        </button>
      </div>
      {providers.length === 0 ? (
        <div style={s.ahaEmptyProvider}>
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
            canRemove={providers.length > 1}
            urlPlaceholder={urlPlaceholder}
            modelPlaceholder={modelPlaceholder}
            hideModelFetch={hideModelFetch}
          />
        ))
      )}
    </section>
  );
}

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
  canRemove,
  urlPlaceholder,
  modelPlaceholder,
  hideModelFetch,
}: {
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
  canRemove: boolean;
  urlPlaceholder: string;
  modelPlaceholder: string;
  hideModelFetch: boolean;
}) {
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const filteredModels = provider.model
    ? modelList.filter((item) => item.toLowerCase().includes(provider.model.toLowerCase()))
    : modelList;
  const canFetch = Boolean(provider.url && provider.apiKey);

  useEffect(() => {
    if (modelList.length > 0) {
      setDropdownOpen(true);
    }
  }, [modelList.length]);

  return (
    <div style={s.ahaProvider}>
      <div style={s.ahaProviderHeader}>
        <button type="button" style={s.ahaProviderTitleButton} onClick={onExpandedChange}>
          <ChevronDown
            size={14}
            style={{
              transform: expanded ? "rotate(0deg)" : "rotate(-90deg)",
              transition: "transform 0.15s",
            }}
          />
          <span style={s.ahaProviderTitle}>Provider {index + 1}</span>
          <span style={s.ahaProviderSummary}>{provider.model || provider.url || "未配置"}</span>
        </button>
        <div style={s.ahaProviderActions}>
          <button
            type="button"
            style={provider.active ? s.ahaActiveBadge : s.ahaInactiveBadge}
            onClick={onActivate}
          >
            {provider.active ? "已激活" : "设为激活"}
          </button>
          <button
            type="button"
            style={{
              ...s.ahaInlineButton,
              color: canRemove ? "var(--danger)" : "var(--text-hint)",
            }}
            onClick={onRemove}
            disabled={!canRemove}
          >
            <Trash2 size={12} />
            删除
          </button>
        </div>
      </div>
      {expanded && (
        <>
          <div style={s.ahaGrid}>
            <label style={s.ahaField}>
              <span style={s.ahaLabel}>URL</span>
              <input
                style={s.ahaInput}
                value={provider.url}
                onChange={(event) => onChange({ url: event.target.value })}
                placeholder={urlPlaceholder}
                spellCheck={false}
              />
            </label>
            <label style={s.ahaField}>
              <span style={s.ahaLabel}>API Key</span>
              <input
                style={s.ahaInput}
                type={showKey ? "text" : "password"}
                value={provider.apiKey}
                onChange={(event) => onChange({ apiKey: event.target.value })}
                placeholder="sk-..."
                spellCheck={false}
              />
            </label>
            <div style={s.ahaField}>
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                  gap: 10,
                }}
              >
                <span style={s.ahaLabel}>模型名称</span>
                {!hideModelFetch && (
                  <button
                    type="button"
                    style={{ ...s.ahaInlineButton, opacity: canFetch && !fetching ? 1 : 0.55 }}
                    onClick={onFetchModels}
                    disabled={!canFetch || fetching}
                  >
                    <RefreshCw size={11} className={fetching ? "spin" : undefined} />
                    {fetching ? "获取中..." : "获取模型"}
                  </button>
                )}
              </div>
              <div style={{ position: "relative" }}>
                <input
                  style={{ ...s.ahaInput, paddingRight: modelList.length > 0 ? 32 : 10 }}
                  value={provider.model}
                  onChange={(event) => {
                    onChange({ model: event.target.value });
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
                    onClick={() => setDropdownOpen((open) => !open)}
                    style={s.ahaDropdownToggle}
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
                  <div style={s.ahaDropdown}>
                    {filteredModels.length === 0 ? (
                      <div
                        style={{
                          padding: "8px 12px",
                          fontSize: 12,
                          color: "var(--text-hint)",
                          textAlign: "center",
                        }}
                      >
                        没有匹配的模型
                      </div>
                    ) : (
                      filteredModels.map((item) => (
                        <button
                          key={item}
                          type="button"
                          style={{
                            ...s.ahaDropdownItem,
                            width: "100%",
                            textAlign: "left",
                            border: "none",
                            color:
                              item === provider.model ? "var(--accent)" : "var(--text-primary)",
                            background:
                              item === provider.model ? "var(--accent-subtle)" : "transparent",
                          }}
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
                <span style={{ ...s.ahaHint, color: "var(--danger)" }}>获取失败：{fetchError}</span>
              )}
            </div>
          </div>
          <div style={s.ahaActionRow}>
            <button type="button" style={s.ahaGhostButton} onClick={onTest} disabled={testing}>
              <Zap size={14} />
              {testing ? "测试中..." : "测试当前 Provider"}
            </button>
            {feedback && (
              <span
                style={{
                  ...s.ahaFeedback,
                  color: feedback.status === "success" ? "var(--success)" : "var(--danger)",
                }}
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

function BehaviorSection({
  autoApprove,
  contextDebug,
  onAutoApproveChange,
  onContextDebugChange,
}: {
  autoApprove: boolean;
  contextDebug: boolean;
  onAutoApproveChange: (value: boolean) => void;
  onContextDebugChange: (value: boolean) => void;
}) {
  return (
    <section style={{ ...s.ahaSection, borderBottom: "none", paddingBottom: 0 }}>
      <div>
        <div style={s.ahaSectionTitle}>行为</div>
        <div style={s.ahaSectionDescription}>
          这些开关不属于模型配置，但会影响 Aha 智能体执行方式。
        </div>
      </div>
      <label style={s.ahaToggleRow}>
        <input
          type="checkbox"
          checked={autoApprove}
          onChange={(event) => onAutoApproveChange(event.target.checked)}
          style={{ accentColor: "var(--accent)", cursor: "pointer", width: 14, height: 14 }}
        />
        <span style={{ fontSize: 12.5, color: "var(--text-primary)" }}>自动批准操作</span>
      </label>
      <span style={{ ...s.ahaHint, marginLeft: 22 }}>
        开启后，Aha 智能体在运行子任务前不再额外请求确认。
      </span>
      <label style={s.ahaToggleRow}>
        <input
          type="checkbox"
          checked={contextDebug}
          onChange={(event) => onContextDebugChange(event.target.checked)}
          style={{ accentColor: "var(--accent)", cursor: "pointer", width: 14, height: 14 }}
        />
        <span style={{ fontSize: 12.5, color: "var(--text-primary)" }}>上下文调试日志</span>
      </label>
      <span style={{ ...s.ahaHint, marginLeft: 22 }}>
        仅在调试时开启。日志文件位于项目根目录的 <code>logs/agent.debug</code>。
      </span>
    </section>
  );
}
