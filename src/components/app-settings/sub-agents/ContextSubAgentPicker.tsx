import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Circle } from "lucide-react";
import type { AgentContext, SubAgentRecord } from "../../../types";
import s from "../../../styles";

export function ContextSubAgentPicker({ context }: { context: AgentContext }) {
  const [allAgents, setAllAgents] = useState<SubAgentRecord[]>([]);
  const [enabledIds, setEnabledIds] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [feedback, setFeedback] = useState("");

  const loadData = useCallback(async () => {
    try {
      const [agents, contextAgents] = await Promise.all([
        invoke<SubAgentRecord[]>("sub_agent_list"),
        invoke<SubAgentRecord[]>("sub_agent_get_context_enabled", { context }),
      ]);
      setAllAgents(agents);
      setEnabledIds(new Set(contextAgents.map((a) => a.id)));
    } catch (e) {
      console.error("load context sub_agents failed:", e);
      setFeedback("加载失败：" + String(e));
    } finally {
      setLoading(false);
    }
  }, [context]);

  useEffect(() => {
    loadData();
  }, [loadData]);

  async function toggle(id: string) {
    const next = new Set(enabledIds);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    setEnabledIds(next);
    setSaving(true);
    try {
      await invoke("sub_agent_set_context_enabled", {
        context,
        subAgentIds: Array.from(next),
      });
      setFeedback("");
    } catch (e) {
      setFeedback("保存失败：" + String(e));
    } finally {
      setSaving(false);
    }
  }

  const contextLabel = context === "project" ? "项目" : "聊天";

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
        <div>
          <span style={{ fontSize: 13, fontWeight: 750 }}>
            {contextLabel}关联子智能体
          </span>
          <span style={{ fontSize: 11, color: "var(--text-muted)", marginLeft: 8 }}>
            勾选后即在此智能体上下文中启用
          </span>
        </div>
        <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
          已选 {enabledIds.size} / {allAgents.length}
        </span>
      </div>

      <div style={{ ...s.ahaBody, flex: 1, overflowY: "auto" }}>
        {loading ? (
          <span style={s.ahaHint}>加载中...</span>
        ) : allAgents.length === 0 ? (
          <span style={s.ahaHint}>暂无子智能体，请先在「子智能体」标签页中创建</span>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            {allAgents.map((record) => {
              const isChecked = enabledIds.has(record.id);
              const isDisabled = !record.enabled;
              return (
                <label
                  key={record.id}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 10,
                    padding: "10px 14px",
                    borderRadius: 8,
                    border: "1px solid var(--border-dim)",
                    background: isChecked ? "var(--bg-card)" : "transparent",
                    cursor: isDisabled ? "not-allowed" : "pointer",
                    opacity: isDisabled ? 0.5 : 1,
                    transition: "background 0.15s",
                  }}
                >
                  {isChecked ? (
                    <Check size={14} style={{ color: "var(--accent, #3b82f6)", flexShrink: 0 }} />
                  ) : (
                    <Circle size={14} style={{ color: "var(--text-muted)", flexShrink: 0 }} />
                  )}
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                      <span style={{ fontSize: 12, fontWeight: 650 }}>{record.name}</span>
                      <span style={{ fontSize: 10, color: "var(--text-muted)" }}>
                        {record.id}
                      </span>
                      {!record.enabled && (
                        <span
                          style={{
                            fontSize: 10,
                            color: "var(--warning, #f59e0b)",
                            fontWeight: 600,
                          }}
                        >
                          (已禁用)
                        </span>
                      )}
                    </div>
                    <div
                      style={{
                        fontSize: 11,
                        color: "var(--text-secondary)",
                        marginTop: 2,
                        lineHeight: 1.4,
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {record.description}
                    </div>
                  </div>
                  <input
                    type="checkbox"
                    checked={isChecked}
                    disabled={isDisabled}
                    onChange={() => !isDisabled && toggle(record.id)}
                    style={{ flexShrink: 0 }}
                  />
                </label>
              );
            })}
          </div>
        )}

        {feedback && (
          <div style={{ padding: "10px 0", fontSize: 12, color: "var(--danger, #ef4444)" }}>
            {feedback}
          </div>
        )}
        {saving && !feedback && (
          <div style={{ padding: "10px 0", fontSize: 11, color: "var(--text-muted)" }}>
            保存中...
          </div>
        )}
      </div>
    </div>
  );
}
