import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Check,
  ChevronDown,
  Circle,
  Eye,
  EyeOff,
  MessageCircle,
  Plus,
  RefreshCw,
  Trash2,
  Wrench,
  type LucideIcon,
  Users,
  Zap,
  FolderKanban,
  MessageSquare,
  Cpu,
  Server,
} from "lucide-react";
import type {
  AhaContextConfig,
  AhaSettingsV2,
  AhaSharedModels,
  AgentContext,
  AgentToolInfo,
  DispatcherModelConfig,
  SshReviewConfig,
} from "../../../types";
import s from "../../../styles";
import { SubAgentManagePanel } from "../sub-agents/SubAgentManagePanel";
import { ContextSubAgentPicker } from "../sub-agents/ContextSubAgentPicker";
import { SshToolPanel } from "./SshToolPanel";

const DEFAULT_SUMMARY_MODEL = "deepseek-v4-flash";
const DEFAULT_IMAGE_MODEL_URL = "https://dashscope.aliyuncs.com/api/v1";
const DEFAULT_IMAGE_MODEL = "qwen-image-2.0-pro";
const DEFAULT_ASR_MODEL = "fun-asr-realtime";

type TopTab = "shared" | "project" | "chat" | "sub_agents" | "ssh_tools";
type AgentSubTab = "models" | "tools" | "sub_agents";
type SharedModelKind = "vision" | "image" | "imageEdit" | "asr" | "tts" | "embedding";
type ContextModelKind = "chat" | "summary";
type Feedback = { status: "success" | "error"; message: string };

type TopTabItem = { key: TopTab; label: string; icon: LucideIcon };
type AgentSubTabItem = { key: AgentSubTab; label: string; icon: LucideIcon };

const TOP_TABS: TopTabItem[] = [
  { key: "shared", label: "通用模型", icon: Cpu },
  { key: "project", label: "项目智能体", icon: FolderKanban },
  { key: "chat", label: "聊天智能体", icon: MessageSquare },
  { key: "ssh_tools", label: "SSH 工具", icon: Server },
  { key: "sub_agents", label: "子智能体", icon: Users },
];

const AGENT_SUB_TABS: AgentSubTabItem[] = [
  { key: "models", label: "主模型", icon: MessageCircle },
  { key: "tools", label: "工具配置", icon: Wrench },
  { key: "sub_agents", label: "子智能体", icon: Users },
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
  const normalized = providers.filter(Boolean).map((p) => cloneModel(p));
  if (normalized.length === 0) return [];
  const activeIndex = normalized.findIndex((p) => p.active);
  return normalized.map((p, i) => ({
    ...p,
    active: activeIndex >= 0 ? i === activeIndex : false,
  }));
}

function emptyProvider(): DispatcherModelConfig {
  return { url: "", apiKey: "", model: "", active: false };
}

function activeProvider(providers: DispatcherModelConfig[]): DispatcherModelConfig {
  return providers.find((p) => p.active) ?? emptyProvider();
}

export function AhaAgentPanel({ projectPath }: { projectPath?: string }) {
  const [activeTopTab, setActiveTopTab] = useState<TopTab>("shared");
  const [activeSubTab, setActiveSubTab] = useState<AgentSubTab>("models");

  const [shared, setShared] = useState<AhaSharedModels>({
    visionModelConfigs: [],
    imageModelConfigs: [],
    imageEditModelConfigs: [],
    asrModelConfigs: [],
    ttsModelConfigs: [],
    embeddingModelConfigs: [],
  });
  const [project, setProject] = useState<AhaContextConfig>({
    chatModelConfigs: [],
    summaryModelConfigs: [],
    allowedTools: [],
  });
  const [chat, setChat] = useState<AhaContextConfig>({
    chatModelConfigs: [],
    summaryModelConfigs: [],
    allowedTools: [],
  });
  const [autoApprove, setAutoApprove] = useState(false);
  const [contextDebug, setContextDebug] = useState(false);
  // SSH 命令审查 AI 配置（编辑入口在 SshToolPanel；此处仅保留以便保存时不覆盖）。
  const [review, setReview] = useState<SshReviewConfig>({
    modelConfig: { url: "", apiKey: "", model: "", active: true },
    systemPrompt: "",
  });

  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [showKey, setShowKey] = useState(false);

  const [expanded, setExpanded] = useState<Record<string, number>>({});
  const [testing, setTesting] = useState<{ key: string; index: number } | null>(null);
  const [feedback, setFeedback] = useState<Record<string, Partial<Record<number, Feedback>>>>({});
  const [modelLists, setModelLists] = useState<Record<string, Partial<Record<number, string[]>>>>(
    {},
  );
  const [fetching, setFetching] = useState<{ key: string; index: number } | null>(null);
  const [fetchError, setFetchError] = useState<Record<string, Partial<Record<number, string>>>>(
    {},
  );

  useEffect(() => {
    invoke<AhaSettingsV2>("aha_get_settings_v2")
      .then((settings) => {
        setShared(settings.shared);
        setProject(settings.project);
        setChat(settings.chat);
        setAutoApprove(settings.autoApproveDispatch);
        setContextDebug(settings.contextDebug);
        if (settings.review) setReview(settings.review);
      })
      .catch(console.error)
      .finally(() => setLoading(false));
  }, []);

  async function handleSave() {
    setSaving(true);
    setSaved(false);
    setSaveError(null);
    try {
      const payload: AhaSettingsV2 = {
        shared,
        project,
        chat,
        autoApproveDispatch: autoApprove,
        contextDebug,
        review,
      };
      const result = await invoke<AhaSettingsV2>("aha_save_settings_v2", { settings: payload });
      setShared(result.shared);
      setProject(result.project);
      setChat(result.chat);
      setAutoApprove(result.autoApproveDispatch);
      setContextDebug(result.contextDebug);
      if (result.review) setReview(result.review);
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2000);
    } catch (error) {
      setSaveError(String(error));
    } finally {
      setSaving(false);
    }
  }

  function updateSharedProviders(
    kind: SharedModelKind,
    updater: (providers: DispatcherModelConfig[]) => DispatcherModelConfig[],
  ) {
    const fieldMap: Record<SharedModelKind, keyof AhaSharedModels> = {
      vision: "visionModelConfigs",
      image: "imageModelConfigs",
      imageEdit: "imageEditModelConfigs",
      asr: "asrModelConfigs",
      tts: "ttsModelConfigs",
      embedding: "embeddingModelConfigs",
    };
    const field = fieldMap[kind];
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
      { url: "", apiKey: "", model: "", active: !providers.some((p) => p.active) },
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
      const kind = key.replace(/-[0-9]+$/, "").replace(/^shared-/, "").replace(/^ctx-[^-]+-/, "");
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
  ) {
    const providers = shared[
      ({
        vision: "visionModelConfigs",
        image: "imageModelConfigs",
        imageEdit: "imageEditModelConfigs",
        asr: "asrModelConfigs",
        tts: "ttsModelConfigs",
        embedding: "embeddingModelConfigs",
      } as Record<SharedModelKind, keyof AhaSharedModels>)[kind]
    ];
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
        onRemove={(index) =>
          updateSharedProviders(kind, (ps) => removeProvider(key, ps, index))
        }
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
  ) {
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

  function renderSharedTab() {
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

  function renderContextModelsTab(context: AgentContext) {
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

  function renderToolsTab(context: AgentContext) {
    const state = context === "project" ? project : chat;
    const setContext = context === "project" ? setProject : setChat;
    return (
      <ToolsTab
        context={context}
        projectPath={projectPath}
        allowedTools={state.allowedTools}
        onChange={(next) => setContext((prev) => ({ ...prev, allowedTools: next }))}
      />
    );
  }

  function renderAgentTab(context: AgentContext) {
    if (activeSubTab === "sub_agents") {
      return <ContextSubAgentPicker context={context} />;
    }
    if (activeSubTab === "tools") {
      return renderToolsTab(context);
    }
    return renderContextModelsTab(context);
  }

  const showSubTabs = activeTopTab === "project" || activeTopTab === "chat";
  const agentContext: AgentContext = activeTopTab === "project" ? "project" : "chat";

  return (
    <>
      <div style={s.ahaPanel}>
        <div style={s.ahaTabs} role="tablist" aria-label="Aha 配置分类">
          {TOP_TABS.map((tab) => {
            const Icon = tab.icon;
            const selected = activeTopTab === tab.key;
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
                onClick={() => {
                  setActiveTopTab(tab.key);
                  if (tab.key !== "shared" && tab.key !== "sub_agents")
                    setActiveSubTab("models");
                }}
              >
                <Icon size={14} />
                {tab.label}
              </button>
            );
          })}
        </div>

        {showSubTabs && (
          <div
            style={{
              display: "flex",
              gap: 4,
              padding: "8px 20px 4px",
              borderBottom: "1px solid var(--border-dim)",
            }}
            role="tablist"
            aria-label="智能体子类"
          >
            {AGENT_SUB_TABS.map((tab) => {
              const Icon = tab.icon;
              const selected = activeSubTab === tab.key;
              return (
                <button
                  key={tab.key}
                  type="button"
                  role="tab"
                  aria-selected={selected}
                  style={{
                    ...s.ahaTab,
                    height: 28,
                    fontSize: 11.5,
                    background: selected ? "var(--bg-subtle)" : "transparent",
                    borderColor: selected ? "var(--border-dim)" : "transparent",
                    color: selected ? "var(--text-primary)" : "var(--text-muted)",
                  }}
                  onClick={() => setActiveSubTab(tab.key)}
                >
                  <Icon size={13} />
                  {tab.label}
                </button>
              );
            })}
          </div>
        )}

        {activeTopTab === "sub_agents" ? (
          <SubAgentManagePanel />
        ) : activeTopTab === "ssh_tools" ? (
          <SshToolPanel projectPath={projectPath} />
        ) : showSubTabs && activeSubTab === "sub_agents" ? (
          <ContextSubAgentPicker context={agentContext} />
        ) : (
          <div style={s.ahaBody}>
            {loading ? (
              <div style={{ color: "var(--text-hint)", fontSize: 13 }}>加载中...</div>
            ) : (
              <div style={s.ahaContent}>
                {activeTopTab !== "shared" && (
                  <div style={{ ...s.ahaActionRow, justifyContent: "flex-end" }}>
                    <button
                      type="button"
                      style={s.ahaGhostButton}
                      onClick={() => setShowKey((v) => !v)}
                    >
                      {showKey ? <EyeOff size={14} /> : <Eye size={14} />}
                      {showKey ? "隐藏 Key" : "显示 Key"}
                    </button>
                  </div>
                )}
                {activeTopTab === "shared" && renderSharedTab()}
                {activeTopTab === "project" && renderAgentTab("project")}
                {activeTopTab === "chat" && renderAgentTab("chat")}
                {activeTopTab !== "shared" && activeSubTab === "models" && (
                  <BehaviorSection
                    autoApprove={autoApprove}
                    contextDebug={contextDebug}
                    onAutoApproveChange={setAutoApprove}
                    onContextDebugChange={setContextDebug}
                  />
                )}
              </div>
            )}
          </div>
        )}
      </div>
      {activeTopTab !== "sub_agents" &&
        activeTopTab !== "ssh_tools" &&
        !(showSubTabs && activeSubTab === "sub_agents") && (
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
      )}
    </>
  );
}

function ToolsTab({
  context,
  projectPath,
  allowedTools,
  onChange,
}: {
  context: AgentContext;
  projectPath?: string;
  allowedTools: string[];
  onChange: (next: string[]) => void;
}) {
  const [availableTools, setAvailableTools] = useState<AgentToolInfo[]>([]);
  const [loadingTools, setLoadingTools] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  // 工具列表较长时可各自折叠；已选默认展开，可选默认折叠。
  const [showSelected, setShowSelected] = useState(true);
  const [showAvailable, setShowAvailable] = useState(false);

  const loadTools = useCallback(async () => {
    setLoadingTools(true);
    setLoadError(null);
    try {
      const tools = await invoke<AgentToolInfo[]>("aha_list_agent_tools", {
        context,
        projectPath: context === "project" ? projectPath ?? null : null,
      });
      setAvailableTools(tools);
    } catch (error) {
      setLoadError(String(error));
    } finally {
      setLoadingTools(false);
    }
  }, [context, projectPath]);

  useEffect(() => {
    loadTools();
  }, [loadTools]);

  const selectedSet = new Set(allowedTools);
  const selectedList = availableTools.filter((t) => selectedSet.has(t.name));
  const unselectedList = availableTools.filter((t) => !selectedSet.has(t.name));

  function toggleTool(name: string) {
    const next = selectedSet.has(name)
      ? allowedTools.filter((t) => t !== name)
      : [...allowedTools, name];
    onChange(next);
  }

  return (
    <div style={s.ahaSection}>
      <div style={s.ahaSectionTitle}>工具配置</div>
      <div style={{ ...s.ahaSectionDescription, marginBottom: 12 }}>
        选择当前智能体可使用的工具。未选择时使用默认工具集；MCP 工具会按当前上下文自动发现。
      </div>
      <div style={{ ...s.ahaActionRow, justifyContent: "space-between", marginBottom: 10 }}>
        <span style={s.ahaHint}>
          {loadingTools ? "正在发现工具..." : `已发现 ${availableTools.length} 个工具`}
          {loadError ? ` · ${loadError}` : ""}
        </span>
        <button type="button" style={s.ahaGhostButton} onClick={loadTools} disabled={loadingTools}>
          <RefreshCw size={13} />
          刷新工具
        </button>
      </div>
      <div style={s.ahaField}>
        <button
          type="button"
          style={{ ...s.ahaCollapsibleTitle, alignItems: "center" }}
          onClick={() => setShowSelected((v) => !v)}
        >
          <ChevronDown
            size={13}
            style={{
              transform: showSelected ? "rotate(0deg)" : "rotate(-90deg)",
              transition: "transform 0.15s",
              flexShrink: 0,
            }}
          />
          <span style={s.ahaLabel}>
            已选工具 ({selectedList.length})
            {selectedList.length === 0 && (
              <span style={{ color: "var(--text-hint)", fontWeight: 400, marginLeft: 8 }}>
                使用默认工具集
              </span>
            )}
          </span>
        </button>
        {showSelected && (
          <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
            {selectedList.length === 0 && (
              <span style={s.ahaHint}>未做任何选择，使用全部默认工具</span>
            )}
            {selectedList.map((tool) => (
              <label
                key={tool.name}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  padding: "6px 8px",
                  borderRadius: 6,
                  background: "var(--bg-subtle)",
                  cursor: "pointer",
                }}
              >
                <input type="checkbox" checked onChange={() => toggleTool(tool.name)} />
                <span style={{ fontSize: 12, fontFamily: "var(--font-mono)" }}>{tool.name}</span>
                <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
                  {tool.description.slice(0, 50)}
                  {tool.description.length > 50 ? "..." : ""}
                </span>
              </label>
            ))}
          </div>
        )}
      </div>
      <div style={s.ahaField}>
        <button
          type="button"
          style={{ ...s.ahaCollapsibleTitle, alignItems: "center" }}
          onClick={() => setShowAvailable((v) => !v)}
        >
          <ChevronDown
            size={13}
            style={{
              transform: showAvailable ? "rotate(0deg)" : "rotate(-90deg)",
              transition: "transform 0.15s",
              flexShrink: 0,
            }}
          />
          <span style={s.ahaLabel}>
            可选工具 ({unselectedList.length})
            {unselectedList.length === 0 && (
              <span style={{ color: "var(--text-hint)", fontWeight: 400, marginLeft: 8 }}>
                无可选项
              </span>
            )}
          </span>
        </button>
        {showAvailable && (
          <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
            {unselectedList.map((tool) => (
              <label
                key={tool.name}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  padding: "6px 8px",
                  borderRadius: 6,
                  cursor: "pointer",
                }}
              >
                <input type="checkbox" checked={false} onChange={() => toggleTool(tool.name)} />
                <span style={{ fontSize: 12, fontFamily: "var(--font-mono)" }}>{tool.name}</span>
                <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
                  {tool.description.slice(0, 50)}
                  {tool.description.length > 50 ? "..." : ""}
                </span>
              </label>
            ))}
          </div>
        )}
      </div>
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
          这些开关影响智能体执行方式（项目和聊天共享）。
        </div>
      </div>
      <label style={s.ahaToggleRow}>
        <input
          type="checkbox"
          checked={autoApprove}
          onChange={(e) => onAutoApproveChange(e.target.checked)}
          style={{ accentColor: "var(--accent)", cursor: "pointer", width: 14, height: 14 }}
        />
        <span style={{ fontSize: 12.5, color: "var(--text-primary)" }}>自动批准操作</span>
      </label>
      <span style={{ ...s.ahaHint, marginLeft: 22 }}>
        开启后，智能体在运行子任务前不再额外请求确认。
      </span>
      <label style={s.ahaToggleRow}>
        <input
          type="checkbox"
          checked={contextDebug}
          onChange={(e) => onContextDebugChange(e.target.checked)}
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
  urlPlaceholder: string;
  modelPlaceholder: string;
  hideModelFetch: boolean;
}) {
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
    <div style={{ ...s.ahaProvider, ...(provider.active ? s.ahaProviderActive : {}) }}>
      <div style={s.ahaProviderHeader}>
        <button type="button" style={s.ahaProviderTitleButton} onClick={onExpandedChange}>
          <ChevronDown
            size={14}
            style={{
              transform: expanded ? "rotate(0deg)" : "rotate(-90deg)",
              transition: "transform 0.15s",
            }}
          />
          <span style={s.ahaProviderTitleWrap}>
            <span style={s.ahaProviderTitle}>Provider {index + 1}</span>
            <span style={s.ahaProviderSummary}>
              {provider.model || provider.url || "未配置模型"}
            </span>
          </span>
        </button>
        <div style={s.ahaProviderActions}>
          <button
            type="button"
            style={provider.active ? s.ahaActiveBadge : s.ahaInactiveBadge}
            onClick={onActivate}
            aria-pressed={provider.active}
            title={provider.active ? "点击后取消激活" : "点击后激活当前 Provider"}
          >
            {provider.active ? <Check size={12} /> : <Circle size={12} />}
            {provider.active ? "已激活" : "激活"}
          </button>
          <button
            type="button"
            style={{ ...s.ahaInlineButton, color: "var(--danger)" }}
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
          <div style={s.ahaGrid}>
            <label style={s.ahaField}>
              <span style={s.ahaLabel}>URL</span>
              <input
                style={s.ahaInput}
                value={provider.url}
                onChange={(e) => onChange({ url: e.target.value })}
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
                onChange={(e) => onChange({ apiKey: e.target.value })}
                placeholder="sk-..."
                spellCheck={false}
              />
            </label>
            <div style={s.ahaField}>
              <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
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
              <div style={{ position: "relative" }} ref={dropdownRef}>
                <input
                  style={{ ...s.ahaInput, paddingRight: modelList.length > 0 ? 32 : 10 }}
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
                      <div style={{ padding: "8px 12px", fontSize: 12, color: "var(--text-hint)" }}>
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
                            color: item === provider.model ? "var(--accent)" : "var(--text-primary)",
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
                <span style={{ ...s.ahaHint, color: "var(--danger)" }}>
                  获取失败：{fetchError}
                </span>
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
