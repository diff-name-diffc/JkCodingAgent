import { memo, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SubAgentRecord } from "../../../types";

export function SubAgentPicker({
  enabledIds,
  onChange,
  title = "关联子智能体",
  description = "选择该分类下的会话可调用的子智能体；未选择时该分类不启用任何子智能体。\n全局启用的子智能体（见「子智能体」标签页）对所有会话仍自动生效。",
}: {
  enabledIds: string[];
  onChange: (next: string[]) => void;
  title?: string;
  description?: string;
}) {
  const [agents, setAgents] = useState<SubAgentRecord[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    invoke<SubAgentRecord[]>("sub_agent_list")
      .then(setAgents)
      .catch((error) => console.error("load sub_agents failed:", error))
      .finally(() => setLoading(false));
  }, []);

  const enabledSet = useMemo(() => new Set(enabledIds), [enabledIds]);

  function toggle(id: string) {
    onChange(
      enabledSet.has(id)
        ? enabledIds.filter((value) => value !== id)
        : [...enabledIds, id],
    );
  }

  return (
    <section className="ai-aha-section">
      <div className="ai-aha-section-title">{title}</div>
      <div className="ai-aha-section-description" style={{ whiteSpace: "pre-line" }}>
        {description}
      </div>
      <div className="ai-aha-hint">
        {loading
          ? "加载子智能体..."
          : `共 ${agents.length} 个子智能体 · 已选 ${enabledIds.length}`}
      </div>

      <div className="ai-aha-tool-list">
        {!loading && agents.length === 0 && (
          <span className="ai-aha-hint">暂无子智能体，可先在「子智能体」标签页创建</span>
        )}
        {agents.map((agent) => (
          <SubAgentOptionRow
            key={agent.id}
            agent={agent}
            checked={enabledSet.has(agent.id)}
            onToggle={() => toggle(agent.id)}
          />
        ))}
      </div>
    </section>
  );
}

const SubAgentOptionRow = memo(function SubAgentOptionRow({
  agent,
  checked,
  onToggle,
}: {
  agent: SubAgentRecord;
  checked: boolean;
  onToggle: () => void;
}) {
  const isDisabled = !agent.enabled;
  return (
    <label className={checked ? "ai-aha-tool-row is-selected" : "ai-aha-tool-row"}>
      <input
        type="checkbox"
        checked={checked}
        disabled={isDisabled}
        onChange={onToggle}
        className="ai-aha-tool-checkbox"
      />
      <span className="ai-aha-tool-text">
        <span className="ai-aha-tool-name">
          {agent.name}
          {!agent.enabled && <span className="ai-aha-tool-name-disabled">(已禁用)</span>}
        </span>
        <span className="ai-aha-tool-description line-clamp-2">
          {agent.description || "暂无描述"}
        </span>
      </span>
    </label>
  );
});
