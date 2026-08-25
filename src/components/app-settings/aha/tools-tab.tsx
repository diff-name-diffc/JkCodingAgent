import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw } from "lucide-react";
import type { AgentContext, AgentToolInfo } from "../../../types";
import { Input } from "../../ui/input";

const SEARCH_DEBOUNCE_MS = 200;
const GENERAL_GROUP = "通用";

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
  const [searchInput, setSearchInput] = useState("");
  const [query, setQuery] = useState("");
  const [selectedOnly, setSelectedOnly] = useState(false);

  // 搜索输入防抖，避免每次击键都重算过滤。
  useEffect(() => {
    const timer = setTimeout(
      () => setQuery(searchInput.trim().toLowerCase()),
      SEARCH_DEBOUNCE_MS,
    );
    return () => clearTimeout(timer);
  }, [searchInput]);

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

  const selectedSet = useMemo(() => new Set(allowedTools), [allowedTools]);

  const toggleTool = useCallback(
    (name: string) => {
      const next = selectedSet.has(name)
        ? allowedTools.filter((t) => t !== name)
        : [...allowedTools, name];
      onChange(next);
    },
    [allowedTools, onChange, selectedSet],
  );

  // 过滤后按 name 前缀分组（如 browser_*）；只有一个成员的前缀并入「通用」。
  const groups = useMemo(() => {
    const visible = availableTools.filter((tool) => {
      if (selectedOnly && !selectedSet.has(tool.name)) return false;
      if (!query) return true;
      return (
        tool.name.toLowerCase().includes(query) ||
        (tool.description ?? "").toLowerCase().includes(query)
      );
    });

    const byPrefix = new Map<string, AgentToolInfo[]>();
    for (const tool of visible) {
      const underscore = tool.name.indexOf("_");
      const prefix = underscore > 0 ? tool.name.slice(0, underscore) : GENERAL_GROUP;
      const list = byPrefix.get(prefix);
      if (list) list.push(tool);
      else byPrefix.set(prefix, [tool]);
    }

    const general: AgentToolInfo[] = [];
    const named: Array<{ title: string; tools: AgentToolInfo[] }> = [];
    for (const [prefix, tools] of byPrefix) {
      if (prefix === GENERAL_GROUP || tools.length === 1) {
        general.push(...tools);
      } else {
        named.push({ title: `${prefix}_*`, tools });
      }
    }
    named.sort((a, b) => a.title.localeCompare(b.title));
    for (const group of named) group.tools.sort((a, b) => a.name.localeCompare(b.name));
    if (general.length > 0) {
      general.sort((a, b) => a.name.localeCompare(b.name));
      named.push({ title: GENERAL_GROUP, tools: general });
    }
    return named;
  }, [availableTools, query, selectedOnly, selectedSet]);

  return (
    <section className="ai-aha-section">
      <div className="ai-aha-section-title">工具配置</div>
      <div className="ai-aha-section-description">
        选择当前智能体可使用的内置工具。未选择时使用默认工具集；MCP 动态工具不在此列，其启停在「MCP 服务器」页管理，配置后始终注入聊天。
      </div>

      <div className="ai-aha-action-row">
        <Input
          value={searchInput}
          onChange={(event) => setSearchInput(event.target.value)}
          placeholder="搜索工具（名称 / 描述）"
          className="h-8 min-w-0 flex-1 text-xs"
          spellCheck={false}
        />
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            role="switch"
            aria-checked={selectedOnly}
            aria-label="仅看已选"
            className={selectedOnly ? "ai-set-switch is-on" : "ai-set-switch"}
            onClick={() => setSelectedOnly((value) => !value)}
          >
            <span className="ai-set-switch-thumb" />
          </button>
          <span className="ai-aha-hint">仅看已选</span>
        </div>
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

      {groups.length === 0 ? (
        <span className="ai-aha-hint">
          {availableTools.length === 0 ? "暂未发现可用工具" : "没有匹配的工具"}
        </span>
      ) : (
        <div className="ai-aha-tool-list">
          {groups.map((group) => (
            <div key={group.title} className="flex flex-col gap-1.5">
              <div className="ai-aha-hint mt-1">
                {group.title} · {group.tools.length}
              </div>
              {group.tools.map((tool) => (
                <ToolOptionRow
                  key={tool.name}
                  tool={tool}
                  checked={selectedSet.has(tool.name)}
                  onToggle={toggleTool}
                />
              ))}
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

const ToolOptionRow = memo(function ToolOptionRow({
  tool,
  checked,
  onToggle,
}: {
  tool: AgentToolInfo;
  checked: boolean;
  onToggle: (name: string) => void;
}) {
  return (
    <label className={checked ? "ai-aha-tool-row is-selected" : "ai-aha-tool-row"}>
      <input
        type="checkbox"
        checked={checked}
        onChange={() => onToggle(tool.name)}
        className="ai-aha-tool-checkbox"
      />
      <span className="ai-aha-tool-text">
        <span className="ai-aha-tool-name">{tool.name}</span>
        <span className="ai-aha-tool-description line-clamp-2">
          {tool.description || "暂无描述"}
        </span>
      </span>
    </label>
  );
});
