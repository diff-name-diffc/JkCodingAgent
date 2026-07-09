import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, RefreshCw } from "lucide-react";
import type { AgentContext, AgentToolInfo } from "../../../types";

export function ToolsTab({
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
    <section className="ai-aha-section">
      <div className="ai-aha-section-title">工具配置</div>
      <div className="ai-aha-section-description">
        选择当前智能体可使用的工具。未选择时使用默认工具集；MCP 工具会按当前上下文自动发现。
      </div>
      <div className="ai-aha-action-row" style={{ justifyContent: "space-between" }}>
        <span className="ai-aha-hint">
          {loadingTools ? "正在发现工具..." : `已发现 ${availableTools.length} 个工具`}
          {loadError ? ` · ${loadError}` : ""}
        </span>
        <button
          type="button"
          className="ai-aha-ghost-button"
          onClick={loadTools}
          disabled={loadingTools}
        >
          <RefreshCw size={13} />
          刷新工具
        </button>
      </div>

      <div className="ai-aha-field">
        <button
          type="button"
          className="ai-aha-collapsible"
          onClick={() => setShowSelected((v) => !v)}
        >
          <ChevronDown
            size={13}
            className="ai-aha-collapsible-chevron"
            style={{ transform: showSelected ? "rotate(0deg)" : "rotate(-90deg)" }}
          />
          <span className="ai-aha-field-label">
            已选工具 ({selectedList.length})
            {selectedList.length === 0 && (
              <span className="ai-aha-collapsible-count">使用默认工具集</span>
            )}
          </span>
        </button>
        {showSelected && (
          <div className="ai-aha-tool-list">
            {selectedList.length === 0 && (
              <span className="ai-aha-hint">未做任何选择，使用全部默认工具</span>
            )}
            {selectedList.map((tool) => (
              <ToolOptionRow
                key={tool.name}
                tool={tool}
                checked
                onToggle={() => toggleTool(tool.name)}
              />
            ))}
          </div>
        )}
      </div>

      <div className="ai-aha-field">
        <button
          type="button"
          className="ai-aha-collapsible"
          onClick={() => setShowAvailable((v) => !v)}
        >
          <ChevronDown
            size={13}
            className="ai-aha-collapsible-chevron"
            style={{ transform: showAvailable ? "rotate(0deg)" : "rotate(-90deg)" }}
          />
          <span className="ai-aha-field-label">
            可选工具 ({unselectedList.length})
            {unselectedList.length === 0 && (
              <span className="ai-aha-collapsible-count">无可选项</span>
            )}
          </span>
        </button>
        {showAvailable && (
          <div className="ai-aha-tool-list">
            {unselectedList.map((tool) => (
              <ToolOptionRow
                key={tool.name}
                tool={tool}
                checked={false}
                onToggle={() => toggleTool(tool.name)}
              />
            ))}
          </div>
        )}
      </div>
    </section>
  );
}

function ToolOptionRow({
  tool,
  checked,
  onToggle,
}: {
  tool: AgentToolInfo;
  checked: boolean;
  onToggle: () => void;
}) {
  return (
    <label className={checked ? "ai-aha-tool-row is-selected" : "ai-aha-tool-row"}>
      <input type="checkbox" checked={checked} onChange={onToggle} className="ai-aha-tool-checkbox" />
      <span className="ai-aha-tool-text">
        <span className="ai-aha-tool-name">{tool.name}</span>
        <span className="ai-aha-tool-description">{tool.description || "暂无描述"}</span>
      </span>
    </label>
  );
}
