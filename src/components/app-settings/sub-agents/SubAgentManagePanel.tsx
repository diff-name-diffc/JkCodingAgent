import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Pencil, Plus, Trash2, Check, X } from "lucide-react";
import type { SubAgentConfig, SubAgentRecord } from "../../../types";
import s from "../../../styles";
import { SubAgentEditorDialog } from "./SubAgentEditorDialog";

export function toBackendConfig(config: SubAgentConfig): string {
  return JSON.stringify({
    agent_id: config.agentId,
    agent_name: config.agentName,
    description: config.description,
    system_prompt: config.systemPrompt,
    user_prompt_template: config.userPromptTemplate,
    allowed_tools: config.allowedTools,
    model_config: {
      inherit_from_parent: config.modelConfig.inheritFromParent,
      api_base: config.modelConfig.apiBase || null,
      api_key: config.modelConfig.apiKey || null,
      model_name: config.modelConfig.modelName || null,
    },
    max_iterations: config.maxIterations,
    max_output_tokens: config.maxOutputTokens,
    temperature: config.temperature,
    timeout_secs: config.timeoutSecs,
    enabled: config.enabled,
    created_at: config.createdAt,
    updated_at: config.updatedAt,
  });
}

export function fromBackendConfig(record: SubAgentRecord): SubAgentConfig | null {
  try {
    const raw = JSON.parse(record.configJson);
    return {
      agentId: raw.agent_id ?? record.id,
      agentName: raw.agent_name ?? record.name,
      description: raw.description ?? record.description,
      systemPrompt: raw.system_prompt ?? "",
      userPromptTemplate: raw.user_prompt_template ?? "{{task}}",
      allowedTools: Array.isArray(raw.allowed_tools)
        ? raw.allowed_tools
        : Array.isArray(raw.allowedTools)
          ? raw.allowedTools
          : [],
      modelConfig: {
        inheritFromParent:
          raw.model_config?.inherit_from_parent ??
          raw.modelConfig?.inheritFromParent ??
          true,
        apiBase: raw.model_config?.api_base ?? raw.modelConfig?.apiBase ?? undefined,
        apiKey: raw.model_config?.api_key ?? raw.modelConfig?.apiKey ?? undefined,
        modelName:
          raw.model_config?.model_name ?? raw.modelConfig?.modelName ?? undefined,
      },
      maxIterations: raw.max_iterations ?? raw.maxIterations ?? 20,
      maxOutputTokens: raw.max_output_tokens ?? raw.maxOutputTokens ?? 4096,
      temperature: raw.temperature ?? 0.7,
      timeoutSecs: raw.timeout_secs ?? raw.timeoutSecs ?? 120,
      enabled: record.enabled,
      createdAt: record.createdAt,
      updatedAt: record.updatedAt,
    };
  } catch {
    return null;
  }
}

export function SubAgentManagePanel() {
  const [agents, setAgents] = useState<SubAgentRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState<{ config: SubAgentConfig | null; isNew: boolean } | null>(
    null,
  );
  const [feedback, setFeedback] = useState("");
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);

  const loadAgents = useCallback(async () => {
    try {
      const list = await invoke<SubAgentRecord[]>("sub_agent_list");
      setAgents(list);
    } catch (e) {
      console.error("load sub_agents failed:", e);
      setFeedback("加载失败：" + String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadAgents();
  }, [loadAgents]);

  async function handleSave(config: SubAgentConfig) {
    const configJson = toBackendConfig(config);
    try {
      if (editing?.isNew) {
        await invoke<SubAgentRecord>("sub_agent_create", { configJson });
      } else if (editing?.config) {
        await invoke<SubAgentRecord>("sub_agent_update", {
          id: editing.config.agentId,
          configJson,
        });
      }
      setEditing(null);
      setFeedback("");
      await loadAgents();
    } catch (e) {
      setFeedback("保存失败：" + String(e));
    }
  }

  async function doDelete(id: string) {
    setDeleting(true);
    try {
      await invoke("sub_agent_delete", { id });
      setConfirmDeleteId(null);
      await loadAgents();
    } catch (e) {
      setFeedback("删除失败：" + String(e));
    } finally {
      setDeleting(false);
    }
  }

  function handleEdit(record: SubAgentRecord) {
    const config = fromBackendConfig(record);
    if (config) {
      setEditing({ config, isNew: false });
    }
  }

  function handleNew() {
    setEditing({ config: null, isNew: true });
  }

  async function handleSeedBrowserAgent() {
    try {
      await invoke("sub_agent_seed_browser");
      setFeedback("");
      await loadAgents();
    } catch (e) {
      setFeedback("恢复失败：" + String(e));
    }
  }

  const hasBrowserAgent = agents.some((a) => a.id === "browser-agent");

  return (
    <div
      style={{
        flex: 1,
        minHeight: 0,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "14px 20px 10px",
          borderBottom: "1px solid var(--border-dim)",
        }}
      >
        <span style={{ fontSize: 13, fontWeight: 750 }}>
          子智能体管理
        </span>
        <button type="button" style={s.ahaGhostButton} onClick={handleNew}>
          <Plus size={14} /> 新建
        </button>
      </div>

      <div style={{ ...s.ahaBody, flex: 1, overflowY: "auto" }}>
        {loading ? (
          <span style={s.ahaHint}>加载中...</span>
        ) : agents.length === 0 ? (
          <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            <span style={s.ahaHint}>暂无子智能体</span>
            <button type="button" style={s.ahaGhostButton} onClick={handleSeedBrowserAgent}>
              恢复内置「浏览器助手」
            </button>
          </div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            {!hasBrowserAgent && (
              <button type="button" style={s.ahaGhostButton} onClick={handleSeedBrowserAgent}>
                恢复内置「浏览器助手」
              </button>
            )}
            {agents.map((record) => {
              const config = fromBackendConfig(record);
              const isConfirming = confirmDeleteId === record.id;
              return (
                <div
                  key={record.id}
                  style={{
                    padding: "12px 16px",
                    borderRadius: 10,
                    border: "1px solid var(--border-dim)",
                    background: "var(--bg-card)",
                  }}
                >
                  <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 6 }}>
                    <span
                      style={{
                        width: 8,
                        height: 8,
                        borderRadius: "50%",
                        background: record.enabled ? "#22c55e" : "#9ca3af",
                        flexShrink: 0,
                      }}
                    />
                    <span style={{ fontSize: 13, fontWeight: 700 }}>{record.name}</span>
                    <span style={{ fontSize: 11, color: "var(--text-muted)" }}>({record.id})</span>
                  </div>
                  <div
                    style={{
                      fontSize: 12,
                      color: "var(--text-secondary)",
                      lineHeight: 1.5,
                      marginBottom: 8,
                    }}
                  >
                    {record.description}
                  </div>
                  <div
                    style={{
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "space-between",
                      marginTop: 4,
                    }}
                  >
                    <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
                      工具: {config?.allowedTools.length ?? 0} 个
                    </span>
                    <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
                      <button
                        type="button"
                        style={s.ahaInlineButton}
                        onClick={() => handleEdit(record)}
                      >
                        <Pencil size={12} /> 编辑
                      </button>
                      {isConfirming ? (
                        <>
                          <span
                            style={{
                              fontSize: 11,
                              color: "var(--danger, #ef4444)",
                              fontWeight: 600,
                            }}
                          >
                            确定删除？
                          </span>
                          <button
                            type="button"
                            style={{
                              ...s.ahaInlineButton,
                              color: "var(--danger, #ef4444)",
                            }}
                            onClick={(e) => {
                              e.stopPropagation();
                              doDelete(record.id);
                            }}
                            disabled={deleting}
                          >
                            <Check size={12} /> 删除
                          </button>
                          <button
                            type="button"
                            style={s.ahaInlineButton}
                            onClick={(e) => {
                              e.stopPropagation();
                              setConfirmDeleteId(null);
                            }}
                          >
                            <X size={12} /> 取消
                          </button>
                        </>
                      ) : (
                        <button
                          type="button"
                          style={{ ...s.ahaInlineButton, color: "var(--danger, #ef4444)" }}
                          onClick={(e) => {
                            e.stopPropagation();
                            setConfirmDeleteId(record.id);
                          }}
                        >
                          <Trash2 size={12} /> 删除
                        </button>
                      )}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        )}

        {feedback && (
          <div style={{ padding: "10px 0", fontSize: 12, color: "var(--danger, #ef4444)" }}>
            {feedback}
          </div>
        )}
      </div>

      {editing && (
        <SubAgentEditorDialog
          config={editing.config}
          isNew={editing.isNew}
          onSave={handleSave}
          onClose={() => setEditing(null)}
        />
      )}
    </div>
  );
}
