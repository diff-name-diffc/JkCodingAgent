import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-react";
import type {
  ModelCategory,
  SubAgentConfig,
  SubAgentToolInfo,
} from "../../../types";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../../ui/select";
import { useAhaSettings } from "../../settings/use-aha-settings";
import { entryLabel } from "../../settings/providers/model-library";
import {
  findMatchedLibraryEntry,
  pickableCategoryOptions,
  pickableEntries,
} from "./sub-agent-model-picker";

type EditorTab = "basic" | "tools" | "runtime";

const MAX_ITERATIONS = 200;
const MIN_OUTPUT_TOKENS = 1024;
const MAX_OUTPUT_TOKENS = 1048576;

interface Props {
  config: SubAgentConfig | null;
  isNew: boolean;
  onSave: (config: SubAgentConfig) => void;
  onClose: () => void;
}

const DEFAULT_CONFIG: SubAgentConfig = {
  agentId: "",
  agentName: "",
  description: "",
  systemPrompt: "",
  userPromptTemplate: "{{task}}",
  allowedTools: [],
  modelConfig: { inheritFromParent: true },
  maxIterations: 60,
  maxOutputTokens: 4096,
  temperature: 0.7,
  timeoutSecs: 3600,
  enabled: true,
  createdAt: 0,
  updatedAt: 0,
};

const TABS: Array<{ key: EditorTab; label: string }> = [
  { key: "basic", label: "基本信息" },
  { key: "tools", label: "工具集配置" },
  { key: "runtime", label: "运行时参数" },
];

export function SubAgentEditorDialog({ config, isNew, onSave, onClose }: Props) {
  const [draft, setDraft] = useState<SubAgentConfig>(config ?? DEFAULT_CONFIG);
  const [activeTab, setActiveTab] = useState<EditorTab>("basic");
  const [availableTools, setAvailableTools] = useState<SubAgentToolInfo[]>([]);
  const [error, setError] = useState("");
  const { settings } = useAhaSettings();
  const modelLibrary = settings?.modelLibrary ?? [];
  // 模型选择器第一步（分类）：初值取当前配置命中的库条目分类，无命中则「对话模型」。
  const [pickerCategory, setPickerCategory] = useState<ModelCategory>(
    () =>
      findMatchedLibraryEntry(modelLibrary, (config ?? DEFAULT_CONFIG).modelConfig)
        ?.category ?? "text",
  );

  useEffect(() => {
    invoke<SubAgentToolInfo[]>("sub_agent_list_tools")
      .then(setAvailableTools)
      .catch((e) => console.error("list_tools failed:", e));
  }, []);

  const selectedTools = new Set(draft.allowedTools);

  const categoryEntries = pickableEntries(modelLibrary, pickerCategory);
  const matchedEntry = findMatchedLibraryEntry(modelLibrary, draft.modelConfig);
  const modelSelectValue =
    matchedEntry && matchedEntry.category === pickerCategory ? matchedEntry.id : "";

  function handlePickCategory(value: string) {
    setPickerCategory(value as ModelCategory);
  }

  function handlePickModel(entryId: string) {
    const picked = categoryEntries.find((entry) => entry.id === entryId);
    if (!picked) return;
    setDraft((d) => ({
      ...d,
      modelConfig: {
        ...d.modelConfig,
        inheritFromParent: false,
        apiBase: picked.url,
        apiKey: picked.apiKey,
        modelName: picked.model,
      },
    }));
  }

  function toggleTool(name: string) {
    setDraft((d) => ({
      ...d,
      allowedTools: selectedTools.has(name)
        ? d.allowedTools.filter((t) => t !== name)
        : [...d.allowedTools, name],
    }));
  }

  function handleSave() {
    if (!draft.agentId.trim()) {
      setError("Agent ID 不能为空");
      return;
    }
    if (draft.agentId.length > 64) {
      setError("Agent ID 长度不能超过 64");
      return;
    }
    if (
      !/^[a-z0-9][a-z0-9_-]*$/.test(draft.agentId)
    ) {
      setError("Agent ID 仅支持小写字母、数字、下划线和短横线，且必须以小写字母或数字开头");
      return;
    }
    if (!draft.agentName.trim()) {
      setError("显示名称不能为空");
      return;
    }
    if (draft.agentName.length > 64) {
      setError("显示名称长度不能超过 64");
      return;
    }
    if (!draft.description.trim()) {
      setError("功能描述不能为空");
      return;
    }
    if (draft.description.length > 512) {
      setError("功能描述长度不能超过 512");
      return;
    }
    if (!draft.systemPrompt.trim()) {
      setError("系统指令不能为空");
      return;
    }
    if (draft.allowedTools.length === 0) {
      setError("至少选择一个工具");
      setActiveTab("tools");
      return;
    }
    if (draft.maxIterations < 1 || draft.maxIterations > MAX_ITERATIONS) {
      setError(`最大迭代轮次必须在 1-${MAX_ITERATIONS} 之间`);
      setActiveTab("runtime");
      return;
    }
    if (
      draft.maxOutputTokens < MIN_OUTPUT_TOKENS ||
      draft.maxOutputTokens > MAX_OUTPUT_TOKENS
    ) {
      setError(`最大输出 Token 必须在 ${MIN_OUTPUT_TOKENS}-${MAX_OUTPUT_TOKENS} 之间`);
      setActiveTab("runtime");
      return;
    }
    if (draft.temperature < 0 || draft.temperature > 2) {
      setError("Temperature 必须在 0-2 之间");
      setActiveTab("runtime");
      return;
    }
    if (draft.timeoutSecs < 1 || draft.timeoutSecs > 3600) {
      setError("超时时间必须在 1-3600 秒之间");
      setActiveTab("runtime");
      return;
    }
    setError("");
    onSave(draft);
  }

  const selectedToolList = availableTools.filter((t) => selectedTools.has(t.name));
  const unselectedToolList = availableTools.filter((t) => !selectedTools.has(t.name));

  return (
    <div className="ai-subagent-dialog-overlay">
      <div className="ai-subagent-dialog">
        <div className="ai-subagent-dialog-header">
          <div className="ai-settings-title-stack">
            <span className="ai-subagent-dialog-title">{isNew ? "新建子智能体" : "编辑子智能体"}</span>
          </div>
          <button type="button" className="ai-settings-close" onClick={onClose} aria-label="关闭">
            <X size={16} />
          </button>
        </div>

        <div className="ai-subagent-dialog-tabs">
          {TABS.map((tab) => (
            <button
              key={tab.key}
              type="button"
              onClick={() => setActiveTab(tab.key)}
              className={activeTab === tab.key ? "ai-aha-tab is-active" : "ai-aha-tab"}
            >
              {tab.label}
            </button>
          ))}
        </div>

        <div className="ai-subagent-dialog-body chat-scroll">
          <div className="ai-rag-content">
            {activeTab === "basic" && (
              <>
                <div className="ai-settings-field-stack">
                  <label className="ai-settings-field-label">Agent ID</label>
                  <input
                    className="ai-settings-input"
                    value={draft.agentId}
                    disabled={!isNew}
                    placeholder="如 browser-agent，仅支持小写字母、数字、短横线和下划线"
                    onChange={(e) =>
                      setDraft((d) => ({ ...d, agentId: e.target.value.toLowerCase() }))
                    }
                  />
                </div>
                <div className="ai-settings-field-stack">
                  <label className="ai-settings-field-label">显示名称</label>
                  <input
                    className="ai-settings-input"
                    value={draft.agentName}
                    placeholder="如 浏览器助手"
                    onChange={(e) => setDraft((d) => ({ ...d, agentName: e.target.value }))}
                  />
                </div>
                <div className="ai-settings-field-stack">
                  <label className="ai-settings-field-label">功能描述</label>
                  <textarea
                    className="ai-settings-textarea ai-subagent-textarea-sm"
                    value={draft.description}
                    placeholder="描述该子智能体的功能和适用场景，会注入主 Agent 的 System Prompt"
                    onChange={(e) => setDraft((d) => ({ ...d, description: e.target.value }))}
                  />
                </div>
                <div className="ai-settings-field-stack">
                  <label className="ai-settings-field-label">系统指令 (System Prompt)</label>
                  <textarea
                    className="ai-settings-textarea ai-subagent-prompt-textarea"
                    value={draft.systemPrompt}
                    placeholder="定义子智能体的角色、行为边界和输出格式"
                    onChange={(e) => setDraft((d) => ({ ...d, systemPrompt: e.target.value }))}
                  />
                </div>
                <div className="ai-settings-field-stack">
                  <label className="ai-settings-field-label">用户输入模板</label>
                  <input
                    className="ai-settings-input"
                    value={draft.userPromptTemplate}
                    placeholder="支持 {{task}} 占位符"
                    onChange={(e) =>
                      setDraft((d) => ({ ...d, userPromptTemplate: e.target.value }))
                    }
                  />
                  <span className="ai-settings-hint">使用 {`{{task}}`} 作为任务描述占位符</span>
                </div>
              </>
            )}

            {activeTab === "tools" && (
              <>
                <div className="ai-settings-field-stack">
                  <label className="ai-settings-field-label">
                    已选工具 ({selectedToolList.length})
                  </label>
                  <div className="ai-subagent-tool-list">
                    {selectedToolList.length === 0 && (
                      <span className="ai-settings-hint">尚未选择任何工具</span>
                    )}
                    {selectedToolList.map((tool) => (
                      <label
                        key={tool.name}
                        className="ai-subagent-tool-row is-selected"
                      >
                        <input
                          type="checkbox"
                          checked
                          onChange={() => toggleTool(tool.name)}
                        />
                        <span className="ai-subagent-tool-name">
                          {tool.name}
                        </span>
                        <span className="ai-subagent-tool-description">
                          {tool.description.slice(0, 40)}
                          {tool.description.length > 40 ? "..." : ""}
                        </span>
                      </label>
                    ))}
                  </div>
                </div>

                <div className="ai-settings-field-stack">
                  <label className="ai-settings-field-label">可选工具</label>
                  <div className="ai-subagent-tool-list">
                    {unselectedToolList.map((tool) => (
                      <label
                        key={tool.name}
                        className="ai-subagent-tool-row"
                      >
                        <input
                          type="checkbox"
                          checked={false}
                          onChange={() => toggleTool(tool.name)}
                        />
                        <span className="ai-subagent-tool-name">
                          {tool.name}
                        </span>
                        <span className="ai-subagent-tool-description">
                          {tool.description.slice(0, 40)}
                          {tool.description.length > 40 ? "..." : ""}
                        </span>
                      </label>
                    ))}
                  </div>
                </div>
              </>
            )}

            {activeTab === "runtime" && (
              <>
                <div className="ai-aha-section">
                  <div className="ai-aha-section-title">模型配置</div>
                  <label className="ai-subagent-choice-row">
                    <input
                      type="radio"
                      name="modelMode"
                      checked={draft.modelConfig.inheritFromParent}
                      onChange={() =>
                        setDraft((d) => ({
                          ...d,
                          modelConfig: { ...d.modelConfig, inheritFromParent: true },
                        }))
                      }
                    />
                    <span>继承主 Agent 配置</span>
                  </label>
                  <label className="ai-subagent-choice-row">
                    <input
                      type="radio"
                      name="modelMode"
                      checked={!draft.modelConfig.inheritFromParent}
                      onChange={() =>
                        setDraft((d) => ({
                          ...d,
                          modelConfig: { ...d.modelConfig, inheritFromParent: false },
                        }))
                      }
                    />
                    <span>自定义配置</span>
                  </label>
                  {!draft.modelConfig.inheritFromParent && (
                    <>
                      <div className="ai-settings-field-stack">
                        <label className="ai-settings-field-label">选择已配置的模型</label>
                        <div className="ai-subagent-model-picker">
                          <Select value={pickerCategory} onValueChange={handlePickCategory}>
                            <SelectTrigger aria-label="模型分类">
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              {pickableCategoryOptions().map((option) => (
                                <SelectItem key={option.category} value={option.category}>
                                  {option.label}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                          {categoryEntries.length === 0 ? (
                            <span className="ai-settings-hint">
                              该分类暂无已配置的启用模型，请前往「模型服务」添加，或在下方手动填写。
                            </span>
                          ) : (
                            <Select value={modelSelectValue} onValueChange={handlePickModel}>
                              <SelectTrigger aria-label="模型">
                                <SelectValue placeholder="选择模型…" />
                              </SelectTrigger>
                              <SelectContent>
                                {categoryEntries.map((entry) => (
                                  <SelectItem key={entry.id} value={entry.id}>
                                    <span className="ai-subagent-model-option">
                                      <span className="ai-subagent-model-option-name">
                                        {entryLabel(entry)}
                                      </span>
                                      {entry.alias?.trim() &&
                                      entry.model.trim() &&
                                      entry.alias.trim() !== entry.model.trim() ? (
                                        <span className="ai-subagent-model-option-model">
                                          {entry.model}
                                        </span>
                                      ) : null}
                                    </span>
                                  </SelectItem>
                                ))}
                              </SelectContent>
                            </Select>
                          )}
                        </div>
                        <span className="ai-settings-hint">
                          选中后自动回填下方 API Base / API Key / 模型名称，可再手动微调。
                        </span>
                      </div>
                      <div className="ai-settings-field-stack">
                        <label className="ai-settings-field-label">API Base</label>
                        <input
                          className="ai-settings-input"
                          value={draft.modelConfig.apiBase ?? ""}
                          placeholder="https://api.openai.com/v1"
                          onChange={(e) =>
                            setDraft((d) => ({
                              ...d,
                              modelConfig: { ...d.modelConfig, apiBase: e.target.value },
                            }))
                          }
                        />
                      </div>
                      <div className="ai-settings-field-stack">
                        <label className="ai-settings-field-label">API Key</label>
                        <input
                          className="ai-settings-input"
                          type="password"
                          value={draft.modelConfig.apiKey ?? ""}
                          onChange={(e) =>
                            setDraft((d) => ({
                              ...d,
                              modelConfig: { ...d.modelConfig, apiKey: e.target.value },
                            }))
                          }
                        />
                      </div>
                      <div className="ai-settings-field-stack">
                        <label className="ai-settings-field-label">模型名称</label>
                        <input
                          className="ai-settings-input"
                          value={draft.modelConfig.modelName ?? ""}
                          placeholder="为空时使用主 Agent 同类型模型"
                          onChange={(e) =>
                            setDraft((d) => ({
                              ...d,
                              modelConfig: { ...d.modelConfig, modelName: e.target.value },
                            }))
                          }
                        />
                      </div>
                    </>
                  )}
                </div>

                <div className="ai-subagent-runtime-grid">
                  <div className="ai-settings-field-stack">
                    <label className="ai-settings-field-label">
                      最大迭代轮次 (1-{MAX_ITERATIONS})
                    </label>
                    <input
                      className="ai-settings-input"
                      type="number"
                      min={1}
                      max={MAX_ITERATIONS}
                      value={draft.maxIterations}
                      onChange={(e) =>
                        setDraft((d) => ({ ...d, maxIterations: Number(e.target.value) }))
                      }
                    />
                  </div>
                  <div className="ai-settings-field-stack">
                    <label className="ai-settings-field-label">
                      最大输出 Token ({MIN_OUTPUT_TOKENS}-{MAX_OUTPUT_TOKENS})
                    </label>
                    <input
                      className="ai-settings-input"
                      type="number"
                      min={MIN_OUTPUT_TOKENS}
                      max={MAX_OUTPUT_TOKENS}
                      value={draft.maxOutputTokens}
                      onChange={(e) =>
                        setDraft((d) => ({ ...d, maxOutputTokens: Number(e.target.value) }))
                      }
                    />
                  </div>
                  <div className="ai-settings-field-stack">
                    <label className="ai-settings-field-label">Temperature (0-2)</label>
                    <input
                      className="ai-settings-input"
                      type="number"
                      min={0}
                      max={2}
                      step={0.1}
                      value={draft.temperature}
                      onChange={(e) =>
                        setDraft((d) => ({ ...d, temperature: Number(e.target.value) }))
                      }
                    />
                  </div>
                  <div className="ai-settings-field-stack">
                    <label className="ai-settings-field-label">超时时间 (1-3600 秒)</label>
                    <input
                      className="ai-settings-input"
                      type="number"
                      min={1}
                      max={3600}
                      value={draft.timeoutSecs}
                      onChange={(e) =>
                        setDraft((d) => ({ ...d, timeoutSecs: Number(e.target.value) }))
                      }
                    />
                  </div>
                </div>
              </>
            )}
          </div>
        </div>

        {error && (
          <div className="ai-subagent-dialog-error">
            {error}
          </div>
        )}

        <div className="ai-subagent-dialog-footer">
          <button type="button" className="ai-secondary-button" onClick={onClose}>
            取消
          </button>
          <button type="button" className="ai-primary-button" onClick={handleSave}>
            {isNew ? "创建" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
