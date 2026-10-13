import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Pencil, Plus, Trash2, Check, X } from "lucide-react";
import type { SubAgentConfig, SubAgentRecord } from "../../../types";
import { BROWSER_AGENT_ID } from "../../../lib/sub-agent-runtime";
import { SubAgentEditorDialog } from "./SubAgentEditorDialog";
import { fromBackendConfig, toBackendConfig } from "./sub-agent-config";

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

  const hasBrowserAgent = agents.some((a) => a.id === BROWSER_AGENT_ID);

  return (
    <div className="ai-subagent-panel">
      <div className="ai-subagent-header">
        <span className="ai-subagent-title">子智能体管理</span>
        <button type="button" className="ai-aha-ghost-button" onClick={handleNew}>
          <Plus size={14} /> 新建
        </button>
      </div>

      <div className="ai-subagent-body chat-scroll">
        {loading ? (
          <span className="ai-settings-hint">加载中...</span>
        ) : agents.length === 0 ? (
          <div className="ai-subagent-empty">
            <span className="ai-settings-hint">暂无子智能体</span>
            <button type="button" className="ai-aha-ghost-button" onClick={handleSeedBrowserAgent}>
              恢复内置「浏览器助手」
            </button>
          </div>
        ) : (
          <div className="ai-subagent-list">
            {!hasBrowserAgent && (
              <button
                type="button"
                className="ai-aha-ghost-button"
                onClick={handleSeedBrowserAgent}
              >
                恢复内置「浏览器助手」
              </button>
            )}
            {agents.map((record) => {
              const config = fromBackendConfig(record);
              const isConfirming = confirmDeleteId === record.id;
              return (
                <div key={record.id} className="ai-subagent-card">
                  <div className="ai-subagent-card-head">
                    <span
                      className={record.enabled ? "ai-subagent-dot is-enabled" : "ai-subagent-dot"}
                    />
                    <span className="ai-subagent-name">{record.name}</span>
                    <span className="ai-subagent-id">({record.id})</span>
                  </div>
                  <div className="ai-subagent-description">{record.description}</div>
                  <div className="ai-subagent-card-footer">
                    <span className="ai-subagent-meta">
                      {record.id === BROWSER_AGENT_ID
                        ? "引擎: browser-use"
                        : `工具: ${config?.allowedTools.length ?? 0} 个`}
                    </span>
                    <div className="ai-subagent-actions">
                      <button
                        type="button"
                        className="ai-subagent-inline-button"
                        onClick={() => handleEdit(record)}
                      >
                        <Pencil size={12} /> 编辑
                      </button>
                      {isConfirming ? (
                        <>
                          <span className="ai-subagent-confirm-text">确定删除？</span>
                          <button
                            type="button"
                            className="ai-subagent-inline-button is-danger"
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
                            className="ai-subagent-inline-button"
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
                          className="ai-subagent-inline-button is-danger"
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

        {feedback && <div className="ai-subagent-feedback">{feedback}</div>}
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
