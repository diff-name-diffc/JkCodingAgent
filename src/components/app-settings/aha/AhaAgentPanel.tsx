import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Check,
  Eye,
  EyeOff,
  type LucideIcon,
  Users,
  FolderKanban,
  MessageSquare,
  Cpu,
  Server,
  MessageCircle,
  Wrench,
} from "lucide-react";
import type {
  AhaContextConfig,
  AhaSettingsV2,
  AhaSharedModels,
  AgentContext,
  ChatCategoryAgentConfig,
  SshReviewConfig,
  SubAgentRecord,
} from "../../../types";
import { SubAgentManagePanel } from "../sub-agents/SubAgentManagePanel";
import { SshToolPanel } from "./SshToolPanel";
import {
  useAhaProviderRegistry,
  withoutChatModelSystemPrompts,
} from "./provider-editor";
import { ToolsTab } from "./tools-tab";
import { ChatCategoryToolsTab } from "./chat-category-tools";
import { SubAgentPicker } from "./sub-agent-picker";

type TopTab = "shared" | "project" | "chat" | "sub_agents" | "ssh_tools";
type AgentSubTab = "models" | "tools" | "sub_agents";

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
  { key: "tools", label: "配置", icon: Wrench },
  { key: "sub_agents", label: "子智能体", icon: Users },
];

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
  const [chatCategoryConfigs, setChatCategoryConfigs] = useState<ChatCategoryAgentConfig[]>([]);
  const [activeChatCategoryId, setActiveChatCategoryId] = useState<string | null>(null);
  // 全局启用的子智能体 ID 列表。对项目和聊天会话都生效；项目会话因无分类，仅由此处驱动。
  const [globalEnabledIds, setGlobalEnabledIds] = useState<string[]>([]);
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

  // Provider 编辑器 UI 状态与远程交互集中在此 hook 内；业务状态 shared/project/chat 由本组件持有。
  const registry = useAhaProviderRegistry({
    shared,
    setShared,
    project,
    setProject,
    chat,
    setChat,
  });

  useEffect(() => {
    Promise.all([
      invoke<AhaSettingsV2>("aha_get_settings_v2"),
      invoke<ChatCategoryAgentConfig[]>("aha_get_chat_category_agent_configs"),
      invoke<SubAgentRecord[]>("sub_agent_get_global_enabled").then((agents) =>
        agents.map((agent) => agent.id),
      ),
    ])
      .then(([settings, categoryConfigs, enabledIds]) => {
        setShared(settings.shared);
        setProject(settings.project);
        setChat(withoutChatModelSystemPrompts(settings.chat));
        setChatCategoryConfigs(categoryConfigs);
        setActiveChatCategoryId((current) => current ?? categoryConfigs[0]?.categoryId ?? null);
        setGlobalEnabledIds(enabledIds);
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
        chat: withoutChatModelSystemPrompts(chat),
        autoApproveDispatch: autoApprove,
        contextDebug,
        review,
      };
      const result = await invoke<AhaSettingsV2>("aha_save_settings_v2", { settings: payload });
      const savedCategoryConfigs = await invoke<ChatCategoryAgentConfig[]>(
        "aha_save_chat_category_agent_configs",
        { configs: chatCategoryConfigs },
      );
      await invoke("sub_agent_set_global_enabled", { subAgentIds: globalEnabledIds });
      setShared(result.shared);
      setProject(result.project);
      setChat(withoutChatModelSystemPrompts(result.chat));
      setChatCategoryConfigs(savedCategoryConfigs);
      setActiveChatCategoryId((current) => {
        if (current && savedCategoryConfigs.some((config) => config.categoryId === current)) {
          return current;
        }
        return savedCategoryConfigs[0]?.categoryId ?? null;
      });
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

  async function reloadChatCategoryConfigs() {
    const configs = await invoke<ChatCategoryAgentConfig[]>("aha_get_chat_category_agent_configs");
    setChatCategoryConfigs(configs);
    setActiveChatCategoryId((current) => {
      if (current && configs.some((config) => config.categoryId === current)) return current;
      return configs[0]?.categoryId ?? null;
    });
  }

  function renderToolsTab(context: AgentContext) {
    if (context === "chat") {
      return (
        <ChatCategoryToolsTab
          configs={chatCategoryConfigs}
          activeCategoryId={activeChatCategoryId}
          onActiveCategoryChange={setActiveChatCategoryId}
          onReload={() => {
            reloadChatCategoryConfigs().catch((error) => setSaveError(String(error)));
          }}
          onChange={(categoryId, patch) =>
            setChatCategoryConfigs((prev) =>
              prev.map((config) =>
                config.categoryId === categoryId ? { ...config, ...patch } : config,
              ),
            )
          }
        />
      );
    }
    return (
      <ToolsTab
        context={context}
        projectPath={projectPath}
        allowedTools={project.allowedTools}
        onChange={(next) => setProject((prev) => ({ ...prev, allowedTools: next }))}
      />
    );
  }

  function renderAgentTab(context: AgentContext) {
    if (activeSubTab === "sub_agents") {
      // 仅项目上下文保留此子标签：项目会话无分类，子智能体启用完全由全局配置驱动。
      // 聊天上下文已隐藏该子标签（其子智能体在「配置」分类页按分类勾选）。
      return (
        <div className="ai-aha-body chat-scroll">
          <SubAgentPicker
            enabledIds={globalEnabledIds}
            title="全局启用子智能体"
            description="勾选的子智能体对所有会话（项目与聊天）自动生效。可在顶部「子智能体」标签页新建或编辑子智能体。"
            onChange={setGlobalEnabledIds}
          />
        </div>
      );
    }
    if (activeSubTab === "tools") {
      return renderToolsTab(context);
    }
    return registry.renderContextModelsTab(context);
  }

  const showSubTabs = activeTopTab === "project" || activeTopTab === "chat";

  return (
    <>
      <div className="ai-aha-panel ai-migrated-aha-panel">
        <div className="ai-aha-tabs" role="tablist" aria-label="Aha 配置分类">
          {TOP_TABS.map((tab) => {
            const Icon = tab.icon;
            const selected = activeTopTab === tab.key;
            return (
              <button
                key={tab.key}
                type="button"
                role="tab"
                aria-selected={selected}
                className={selected ? "ai-aha-tab is-active" : "ai-aha-tab"}
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
          <div className="ai-aha-subtabs" role="tablist" aria-label="智能体子类">
            {AGENT_SUB_TABS.filter((tab) =>
              activeTopTab === "chat" ? tab.key !== "sub_agents" : true,
            ).map((tab) => {
              const Icon = tab.icon;
              const selected = activeSubTab === tab.key;
              return (
                <button
                  key={tab.key}
                  type="button"
                  role="tab"
                  aria-selected={selected}
                  className={
                    selected
                      ? "ai-aha-tab ai-aha-subtab is-active"
                      : "ai-aha-tab ai-aha-subtab"
                  }
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
        ) : (
          <div className="ai-aha-body chat-scroll">
            {loading ? (
              <div className="ai-settings-empty">加载中...</div>
            ) : (
              <div className="ai-aha-content">
                {activeTopTab !== "shared" && (
                  <div className="ai-aha-action-row is-end">
                    <button
                      type="button"
                      className="ai-aha-ghost-button"
                      onClick={() => registry.setShowKey((v) => !v)}
                    >
                      {registry.showKey ? <EyeOff size={14} /> : <Eye size={14} />}
                      {registry.showKey ? "隐藏 Key" : "显示 Key"}
                    </button>
                  </div>
                )}
                {activeTopTab === "shared" && registry.renderSharedTab()}
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
          <div className="ai-settings-footer ai-aha-footer">
            {saveError && <span className="ai-aha-feedback is-error">{saveError}</span>}
            {saved && (
              <span className="ai-settings-saved">
                <Check size={12} /> 已保存
              </span>
            )}
            <button
              type="button"
              className="ai-primary-button"
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
    <section className="ai-aha-section is-last">
      <div>
        <div className="ai-aha-section-title">行为</div>
        <div className="ai-aha-section-description">
          这些开关影响智能体执行方式（项目和聊天共享）。
        </div>
      </div>
      <label className="ai-aha-toggle-row">
        <input
          type="checkbox"
          checked={autoApprove}
          onChange={(e) => onAutoApproveChange(e.target.checked)}
          className="ai-aha-toggle-checkbox"
        />
        <span>自动批准操作</span>
      </label>
      <span className="ai-aha-hint" style={{ marginLeft: 22 }}>
        开启后，智能体在运行子任务前不再额外请求确认。
      </span>
      <label className="ai-aha-toggle-row">
        <input
          type="checkbox"
          checked={contextDebug}
          onChange={(e) => onContextDebugChange(e.target.checked)}
          className="ai-aha-toggle-checkbox"
        />
        <span>上下文调试日志</span>
      </label>
      <span className="ai-aha-hint" style={{ marginLeft: 22 }}>
        仅在调试时开启。日志文件位于项目根目录的 <code>logs/agent.debug</code>。
      </span>
    </section>
  );
}
