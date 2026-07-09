import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown } from "lucide-react";
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
  const [showAvailable, setShowAvailable] = useState(false);

  useEffect(() => {
    invoke<SubAgentRecord[]>("sub_agent_list")
      .then(setAgents)
      .catch((error) => console.error("load sub_agents failed:", error))
      .finally(() => setLoading(false));
  }, []);

  const enabledSet = new Set(enabledIds);
  const selectedList = agents.filter((agent) => enabledSet.has(agent.id));
  const availableList = agents.filter((agent) => !enabledSet.has(agent.id));

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
        {loading ? "加载子智能体..." : `共 ${agents.length} 个子智能体`}
      </div>

      <div className="ai-aha-field">
        <button
          type="button"
          className="ai-aha-collapsible"
          onClick={() => setShowAvailable((value) => !value)}
        >
          <ChevronDown
            size={13}
            className="ai-aha-collapsible-chevron"
            style={{ transform: showAvailable ? "rotate(0deg)" : "rotate(-90deg)" }}
          />
          <span className="ai-aha-field-label">
            可选子智能体 ({availableList.length})
            {availableList.length === 0 && (
              <span className="ai-aha-collapsible-count">无可选项</span>
            )}
          </span>
        </button>
        {showAvailable && (
          <div className="ai-aha-tool-list">
            {loading && <span className="ai-aha-hint">加载中...</span>}
            {availableList.map((agent) => (
              <SubAgentOptionRow
                key={agent.id}
                agent={agent}
                checked={false}
                onToggle={() => toggle(agent.id)}
              />
            ))}
          </div>
        )}
      </div>

      <div className="ai-aha-field">
        <div className="ai-aha-collapsible is-static">
          <ChevronDown size={13} className="ai-aha-collapsible-chevron" />
          <span className="ai-aha-field-label">
            已选子智能体 ({selectedList.length})
            {selectedList.length === 0 && (
              <span className="ai-aha-collapsible-count">该分类暂未启用子智能体</span>
            )}
          </span>
        </div>
        <div className="ai-aha-tool-list">
          {selectedList.length === 0 && (
            <span className="ai-aha-hint">展开上方「可选子智能体」进行勾选</span>
          )}
          {selectedList.map((agent) => (
            <SubAgentOptionRow
              key={agent.id}
              agent={agent}
              checked
              onToggle={() => toggle(agent.id)}
            />
          ))}
        </div>
      </div>
    </section>
  );
}

function SubAgentOptionRow({
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
        <span className="ai-aha-tool-description">{agent.description || "暂无描述"}</span>
      </span>
    </label>
  );
}
