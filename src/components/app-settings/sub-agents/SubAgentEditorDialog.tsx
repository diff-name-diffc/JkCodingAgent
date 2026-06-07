import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-react";
import type { SubAgentConfig, SubAgentToolInfo } from "../../../types";
import s from "../../../styles";

type EditorTab = "basic" | "tools" | "runtime";

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
  maxIterations: 20,
  maxOutputTokens: 4096,
  temperature: 0.7,
  timeoutSecs: 120,
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

  useEffect(() => {
    invoke<SubAgentToolInfo[]>("sub_agent_list_tools")
      .then(setAvailableTools)
      .catch((e) => console.error("list_tools failed:", e));
  }, []);

  const selectedTools = new Set(draft.allowedTools);

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
    if (!draft.agentName.trim()) {
      setError("显示名称不能为空");
      return;
    }
    if (!draft.description.trim()) {
      setError("功能描述不能为空");
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
    setError("");
    onSave(draft);
  }

  const selectedToolList = availableTools.filter((t) => selectedTools.has(t.name));
  const unselectedToolList = availableTools.filter((t) => !selectedTools.has(t.name));

  return (
    <div style={s.modalOverlay}>
      <div
        style={{
          ...s.compactDialogBox,
          width: 620,
          maxWidth: "92vw",
          maxHeight: "88vh",
        }}
      >
        <div style={s.compactDialogHeader}>
          <div style={s.compactDialogTitleBlock}>
            <span style={s.compactDialogTitle}>{isNew ? "新建子智能体" : "编辑子智能体"}</span>
          </div>
          <button type="button" style={s.modalCloseBtn} onClick={onClose} aria-label="关闭">
            <X size={16} />
          </button>
        </div>

        <div style={{ display: "flex", gap: 4, padding: "8px 20px 0" }}>
          {TABS.map((tab) => (
            <button
              key={tab.key}
              type="button"
              onClick={() => setActiveTab(tab.key)}
              style={{
                ...s.ahaTab,
                background: activeTab === tab.key ? "var(--bg-hover)" : "transparent",
                color: activeTab === tab.key ? "var(--text-primary)" : "var(--text-muted)",
              }}
            >
              {tab.label}
            </button>
          ))}
        </div>

        <div style={{ ...s.compactDialogBody, flex: 1, overflowY: "auto" }}>
          <div style={s.ahaContent}>
            {activeTab === "basic" && (
              <>
                <div style={s.ahaField}>
                  <label style={s.ahaLabel}>Agent ID</label>
                  <input
                    style={{ ...s.ahaInput, opacity: isNew ? 1 : 0.6 }}
                    value={draft.agentId}
                    disabled={!isNew}
                    placeholder="如 browser-agent，仅支持小写字母、数字、短横线和下划线"
                    onChange={(e) =>
                      setDraft((d) => ({ ...d, agentId: e.target.value.toLowerCase() }))
                    }
                  />
                </div>
                <div style={s.ahaField}>
                  <label style={s.ahaLabel}>显示名称</label>
                  <input
                    style={s.ahaInput}
                    value={draft.agentName}
                    placeholder="如 浏览器助手"
                    onChange={(e) => setDraft((d) => ({ ...d, agentName: e.target.value }))}
                  />
                </div>
                <div style={s.ahaField}>
                  <label style={s.ahaLabel}>功能描述</label>
                  <textarea
                    style={{ ...s.ahaInput, height: 60, padding: "8px 10px", resize: "vertical" }}
                    value={draft.description}
                    placeholder="描述该子智能体的功能和适用场景，会注入主 Agent 的 System Prompt"
                    onChange={(e) => setDraft((d) => ({ ...d, description: e.target.value }))}
                  />
                </div>
                <div style={s.ahaField}>
                  <label style={s.ahaLabel}>系统指令 (System Prompt)</label>
                  <textarea
                    style={{
                      ...s.ahaInput,
                      height: 180,
                      padding: "8px 10px",
                      resize: "vertical",
                      fontFamily: "var(--font-mono)",
                      fontSize: 11.5,
                      lineHeight: 1.55,
                    }}
                    value={draft.systemPrompt}
                    placeholder="定义子智能体的角色、行为边界和输出格式"
                    onChange={(e) => setDraft((d) => ({ ...d, systemPrompt: e.target.value }))}
                  />
                </div>
                <div style={s.ahaField}>
                  <label style={s.ahaLabel}>用户输入模板</label>
                  <input
                    style={s.ahaInput}
                    value={draft.userPromptTemplate}
                    placeholder="支持 {{task}} 占位符"
                    onChange={(e) =>
                      setDraft((d) => ({ ...d, userPromptTemplate: e.target.value }))
                    }
                  />
                  <span style={s.ahaHint}>使用 {`{{task}}`} 作为任务描述占位符</span>
                </div>
              </>
            )}

            {activeTab === "tools" && (
              <>
                <div style={s.ahaField}>
                  <label style={s.ahaLabel}>
                    已选工具 ({selectedToolList.length})
                  </label>
                  <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
                    {selectedToolList.length === 0 && (
                      <span style={s.ahaHint}>尚未选择任何工具</span>
                    )}
                    {selectedToolList.map((tool) => (
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
                        <input
                          type="checkbox"
                          checked
                          onChange={() => toggleTool(tool.name)}
                        />
                        <span style={{ fontSize: 12, fontFamily: "var(--font-mono)" }}>
                          {tool.name}
                        </span>
                        <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
                          {tool.description.slice(0, 40)}
                          {tool.description.length > 40 ? "..." : ""}
                        </span>
                      </label>
                    ))}
                  </div>
                </div>

                <div style={s.ahaField}>
                  <label style={s.ahaLabel}>可选工具</label>
                  <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
                    {unselectedToolList.map((tool) => (
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
                        <input
                          type="checkbox"
                          checked={false}
                          onChange={() => toggleTool(tool.name)}
                        />
                        <span style={{ fontSize: 12, fontFamily: "var(--font-mono)" }}>
                          {tool.name}
                        </span>
                        <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
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
                <div style={s.ahaSection}>
                  <div style={s.ahaSectionTitle}>模型配置</div>
                  <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
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
                    <span style={{ fontSize: 12 }}>继承主 Agent 配置</span>
                  </label>
                  <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
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
                    <span style={{ fontSize: 12 }}>自定义配置</span>
                  </label>
                  {!draft.modelConfig.inheritFromParent && (
                    <>
                      <div style={s.ahaField}>
                        <label style={s.ahaLabel}>API Base</label>
                        <input
                          style={s.ahaInput}
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
                      <div style={s.ahaField}>
                        <label style={s.ahaLabel}>API Key</label>
                        <input
                          style={s.ahaInput}
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
                      <div style={s.ahaField}>
                        <label style={s.ahaLabel}>模型名称</label>
                        <input
                          style={s.ahaInput}
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

                <div style={s.ahaGrid}>
                  <div style={s.ahaField}>
                    <label style={s.ahaLabel}>最大迭代轮次 (1-100)</label>
                    <input
                      style={s.ahaInput}
                      type="number"
                      min={1}
                      max={100}
                      value={draft.maxIterations}
                      onChange={(e) =>
                        setDraft((d) => ({ ...d, maxIterations: Number(e.target.value) }))
                      }
                    />
                  </div>
                  <div style={s.ahaField}>
                    <label style={s.ahaLabel}>最大输出 Token (256-65536)</label>
                    <input
                      style={s.ahaInput}
                      type="number"
                      min={256}
                      max={65536}
                      value={draft.maxOutputTokens}
                      onChange={(e) =>
                        setDraft((d) => ({ ...d, maxOutputTokens: Number(e.target.value) }))
                      }
                    />
                  </div>
                  <div style={s.ahaField}>
                    <label style={s.ahaLabel}>Temperature (0-2)</label>
                    <input
                      style={s.ahaInput}
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
                  <div style={s.ahaField}>
                    <label style={s.ahaLabel}>超时时间 (秒，10-600)</label>
                    <input
                      style={s.ahaInput}
                      type="number"
                      min={10}
                      max={600}
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
          <div style={{ padding: "0 20px", fontSize: 12, color: "var(--danger, #ef4444)" }}>
            {error}
          </div>
        )}

        <div style={s.compactDialogFooter}>
          <button type="button" style={s.modalCancelBtn} onClick={onClose}>
            取消
          </button>
          <button type="button" style={s.modalSaveBtn} onClick={handleSave}>
            {isNew ? "创建" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
