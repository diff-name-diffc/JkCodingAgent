import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, ChevronRight, RefreshCw, X } from "lucide-react";
import type { AgentContext, AgentToolInfo, McpServerState, McpStatus } from "../../../types";
import { isMcpToolName } from "../../../lib/mcp-category-tools";
import { Input } from "../../ui/input";

const SEARCH_DEBOUNCE_MS = 200;
const GENERAL_GROUP = "通用";

function mcpStateColor(state: McpServerState): string {
  switch (state) {
    case "disabled":
      return "var(--text-hint)";
    case "healthy":
      return "var(--success)";
    case "invalid_config":
    case "connection_failed":
      return "var(--danger)";
    case "spawn_failed":
      return "var(--warning)";
  }
}

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
  const isChat = context === "chat";
  const [availableTools, setAvailableTools] = useState<AgentToolInfo[]>([]);
  const [loadingTools, setLoadingTools] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [mcpStatus, setMcpStatus] = useState<McpStatus | null>(null);
  const [loadingMcp, setLoadingMcp] = useState(false);
  const [mcpError, setMcpError] = useState<string | null>(null);
  const [builtinCollapsed, setBuiltinCollapsed] = useState(false);
  const [mcpCollapsed, setMcpCollapsed] = useState(false);
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

  // MCP 清单走新鲜窗口命令：聊天 run 已预热缓存时直接复用，
  // 避免每次打开设置页都拉起全部服务器进程。
  const loadMcpStatus = useCallback(async () => {
    setLoadingMcp(true);
    setMcpError(null);
    try {
      setMcpStatus(await invoke<McpStatus>("mcp_global_status_recent"));
    } catch (error) {
      setMcpError(String(error));
    } finally {
      setLoadingMcp(false);
    }
  }, []);

  useEffect(() => {
    loadTools();
    if (isChat) loadMcpStatus();
  }, [loadTools, loadMcpStatus, isChat]);

  const refreshAll = useCallback(() => {
    loadTools();
    if (isChat) loadMcpStatus();
  }, [loadTools, loadMcpStatus, isChat]);

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

  const matchesFilters = useCallback(
    (name: string, description: string) => {
      if (selectedOnly && !selectedSet.has(name)) return false;
      if (!query) return true;
      return (
        name.toLowerCase().includes(query) || description.toLowerCase().includes(query)
      );
    },
    [query, selectedOnly, selectedSet],
  );

  // 内置工具按 name 前缀分组（如 browser_*）；只有一个成员的前缀并入「通用」。
  const groups = useMemo(() => {
    const visible = availableTools.filter((tool) =>
      matchesFilters(tool.name, tool.description ?? ""),
    );

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
  }, [availableTools, matchesFilters]);

  // MCP 工具按服务器分组；过滤与内置工具共用搜索/仅看已选规则。
  const mcpGroups = useMemo(() => {
    if (!isChat || !mcpStatus) return [];
    return mcpStatus.servers
      .map((server) => ({
        server,
        tools: server.tools.filter((tool) =>
          matchesFilters(tool.exposedName, tool.description),
        ),
      }))
      .filter((group) => group.tools.length > 0);
  }, [isChat, mcpStatus, matchesFilters]);

  const mcpToolTotal = useMemo(
    () => (mcpStatus ? mcpStatus.servers.reduce((sum, server) => sum + server.tools.length, 0) : 0),
    [mcpStatus],
  );

  // 已勾选但当前状态里不存在的 MCP 名字（服务器/工具改名或被移除），
  // 列出来供一键移除，避免名单里留下永远匹配不到的幽灵条目。
  const staleMcpNames = useMemo(() => {
    if (!isChat || !mcpStatus) return [];
    const known = new Set(
      mcpStatus.servers.flatMap((server) => server.tools.map((tool) => tool.exposedName)),
    );
    return allowedTools.filter((name) => isMcpToolName(name) && !known.has(name));
  }, [isChat, mcpStatus, allowedTools]);

  const builtinSection = (
    <>
      {isChat && (
        <ToolSectionHeader
          title="普通工具"
          count={availableTools.length}
          hint={loadingTools ? "正在发现工具..." : undefined}
          collapsed={builtinCollapsed}
          onToggle={() => setBuiltinCollapsed((value) => !value)}
        />
      )}
      {!builtinCollapsed &&
        (groups.length === 0 ? (
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
        ))}
    </>
  );

  return (
    <section className="ai-aha-section">
      <div className="ai-aha-section-title">工具配置</div>
      <div className="ai-aha-section-description">
        {isChat
          ? "选择该分类可使用的工具。普通工具未选择时使用默认工具集；MCP 工具仅在显式勾选后注入该分类的聊天，服务器级启停在「MCP 服务器」页管理。"
          : "选择当前智能体可使用的内置工具。未选择时使用默认工具集。"}
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
        {!isChat && (
          <span className="ai-aha-hint">
            {loadingTools ? "正在发现工具..." : `已发现 ${availableTools.length} 个工具`}
            {loadError ? ` · ${loadError}` : ""}
          </span>
        )}
        <button
          type="button"
          className="ai-aha-ghost-button"
          onClick={refreshAll}
          disabled={loadingTools || loadingMcp}
        >
          <RefreshCw size={13} />
          刷新工具
        </button>
      </div>

      {builtinSection}

      {isChat && (
        <>
          <ToolSectionHeader
            title="MCP 工具"
            count={mcpToolTotal}
            hint={
              loadingMcp
                ? "正在加载 MCP 清单..."
                : mcpError
                  ? mcpError
                  : mcpStatus && mcpToolTotal === 0
                    ? "全局未配置可用的 MCP 工具"
                    : undefined
            }
            collapsed={mcpCollapsed}
            onToggle={() => setMcpCollapsed((value) => !value)}
          />
          {!mcpCollapsed && (
            <>
              {mcpGroups.length === 0 ? (
                !loadingMcp &&
                !mcpError &&
                mcpToolTotal > 0 && (
                  <span className="ai-aha-hint">没有匹配的 MCP 工具</span>
                )
              ) : (
                <div className="ai-aha-tool-list">
                  {mcpGroups.map(({ server, tools }) => (
                    <div key={server.name} className="flex flex-col gap-1.5">
                      <div className="ai-aha-mcp-server-label">
                        <span
                          className="ai-aha-mcp-dot"
                          style={{ background: mcpStateColor(server.state) }}
                        />
                        <span>{server.name}</span>
                        <span>· {tools.length}</span>
                        {server.state !== "healthy" && <span>（{server.summary}）</span>}
                      </div>
                      {tools.map((tool) => (
                        <ToolOptionRow
                          key={tool.exposedName}
                          tool={{ name: tool.exposedName, description: tool.description }}
                          checked={selectedSet.has(tool.exposedName)}
                          onToggle={toggleTool}
                        />
                      ))}
                    </div>
                  ))}
                </div>
              )}
              {staleMcpNames.length > 0 && (
                <div className="ai-aha-tool-list mt-2">
                  <div className="ai-aha-hint">已失效的 MCP 工具（服务器或工具已变更）</div>
                  {staleMcpNames.map((name) => (
                    <div key={name} className="ai-aha-mcp-stale-row">
                      <span className="ai-aha-mcp-stale-name">{name}</span>
                      <button
                        type="button"
                        className="ai-aha-ghost-button"
                        title="从允许列表移除"
                        onClick={() => toggleTool(name)}
                      >
                        <X size={12} />
                        移除
                      </button>
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </>
      )}
    </section>
  );
}

function ToolSectionHeader({
  title,
  count,
  hint,
  collapsed,
  onToggle,
}: {
  title: string;
  count: number;
  hint?: string;
  collapsed: boolean;
  onToggle: () => void;
}) {
  return (
    <button type="button" className="ai-aha-tool-section-header" onClick={onToggle}>
      {collapsed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
      <span>{title}</span>
      <span className="ai-aha-tool-section-count">· {count}</span>
      {hint && <span className="ai-aha-tool-section-hint">{hint}</span>}
    </button>
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
