import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, Plug, Plus, RefreshCw, Trash2 } from "lucide-react";
import type { McpConfig, McpServerConfig } from "../../../types";
import { cn } from "../../../lib/cn";
import { ConfirmDialog } from "../ConfirmDialog";
import { EmptyState } from "../EmptyState";
import { FieldLabel } from "../FieldLabel";
import { Section } from "../Section";
import { toast } from "../toast";

const AUTOSAVE_DELAY_MS = 400;

type TransportKind = "stdio" | "streamable_http" | "unix_socket_http";

const TRANSPORT_LABELS: Record<TransportKind, string> = {
  stdio: "本地进程（stdio）",
  streamable_http: "HTTP（streamable）",
  unix_socket_http: "Unix socket（HTTP）",
};

interface McpEntry {
  name: string;
  server: McpServerConfig;
}

const EMPTY_SERVER: McpServerConfig = {
  enabled: true,
  transport: "stdio",
  command: "",
  args: [],
  env: {},
  headers: {},
};

/** 条目列表 ↔ mcpServers Record 的双向转换（保持插入顺序）。 */
function toEntries(config: McpConfig): McpEntry[] {
  return Object.entries(config.mcpServers ?? {}).map(([name, server]) => ({
    name,
    server: {
      ...server,
      args: server.args ?? [],
      env: server.env ?? {},
      headers: server.headers ?? {},
    },
  }));
}

function toConfig(entries: McpEntry[]): McpConfig {
  const mcpServers: Record<string, McpServerConfig> = {};
  for (const { name, server } of entries) {
    const key = name.trim();
    if (!key) continue;
    mcpServers[key] = server;
  }
  return { mcpServers };
}

function parseLines(raw: string): string[] {
  return raw
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

function parseKeyValueLines(raw: string, separator: RegExp): Record<string, string> {
  const map: Record<string, string> = {};
  for (const line of parseLines(raw)) {
    const match = separator.exec(line);
    if (match && match[1]?.trim()) {
      map[match[1].trim()] = line.slice(match[0].length).trim();
    }
  }
  return map;
}

function nextServerName(entries: McpEntry[]): string {
  let index = entries.length + 1;
  let name = `mcp-${index}`;
  const used = new Set(entries.map((entry) => entry.name));
  while (used.has(name)) {
    index += 1;
    name = `mcp-${index}`;
  }
  return name;
}

type EditorMode = "form" | "json";

interface ParsedConfigText {
  config: McpConfig | null;
  error: string | null;
}

/**
 * 解析用户编辑的配置 JSON。顶层必须是对象；`mcpServers` 缺失视为空注册表，
 * 兼容后端别名键 `servers`（与项目级 mcp.json 同形状）。字段级形状交给
 * 后端 serde 校验——客户端只做结构防护，避免把非法文本送进保存流程。
 */
function parseConfigText(text: string): ParsedConfigText {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (err) {
    return {
      config: null,
      error: `JSON 解析失败：${err instanceof Error ? err.message : String(err)}`,
    };
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return { config: null, error: "配置顶层必须是一个 JSON 对象" };
  }
  const root = parsed as Record<string, unknown>;
  const servers = root.mcpServers ?? root.servers;
  if (
    servers !== undefined &&
    (typeof servers !== "object" || servers === null || Array.isArray(servers))
  ) {
    return { config: null, error: "mcpServers 必须是一个 JSON 对象（服务器名 → 服务器配置）" };
  }
  for (const [name, server] of Object.entries((servers as Record<string, unknown>) ?? {})) {
    if (typeof server !== "object" || server === null || Array.isArray(server)) {
      return { config: null, error: `服务器「${name}」的配置必须是一个 JSON 对象` };
    }
  }
  return {
    config: { mcpServers: (servers as Record<string, McpServerConfig>) ?? {} },
    error: null,
  };
}

function serializeConfig(config: McpConfig): string {
  return `${JSON.stringify(config, null, 2)}\n`;
}

/**
 * 设置弹窗的「MCP 服务器」页：全局 MCP 注册表编辑器。
 * 全局服务器对所有项目与聊天生效；项目可在自身 mcp.json 中定义同名服务器覆盖。
 * 支持表单与 JSON 两种编辑模式，共享同一份条目状态与自动保存管线。
 */
export function McpServersPage() {
  const [entries, setEntries] = useState<McpEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [pendingDeleteIndex, setPendingDeleteIndex] = useState<number | null>(null);
  const [mode, setMode] = useState<EditorMode>("form");
  const [jsonText, setJsonText] = useState("");
  const [jsonError, setJsonError] = useState<string | null>(null);

  const entriesRef = useRef(entries);
  entriesRef.current = entries;
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savingRef = useRef(false);

  const loadConfig = useCallback(async (): Promise<McpConfig | null> => {
    setLoading(true);
    setLoadError(null);
    try {
      const config = await invoke<McpConfig>("mcp_global_config_get");
      setEntries(toEntries(config));
      return config;
    } catch (err) {
      setLoadError(String(err));
      return null;
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadConfig();
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, [loadConfig]);

  const saveNow = useCallback(async () => {
    if (savingRef.current) return;
    savingRef.current = true;
    try {
      const saved = await invoke<McpConfig>("mcp_global_config_save", {
        config: toConfig(entriesRef.current),
      });
      setEntries(toEntries(saved));
      setSaveError(null);
    } catch (err) {
      setSaveError(String(err));
      toast.error(`保存失败：${String(err)}`);
    } finally {
      savingRef.current = false;
    }
  }, []);

  const scheduleSave = useCallback(() => {
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => {
      timerRef.current = null;
      void saveNow();
    }, AUTOSAVE_DELAY_MS);
  }, [saveNow]);

  function updateEntry(index: number, updater: (entry: McpEntry) => McpEntry) {
    setEntries((prev) => prev.map((entry, i) => (i === index ? updater(entry) : entry)));
    scheduleSave();
  }

  function toggleExpand(index: number) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  }

  function addServer() {
    const newIndex = entries.length;
    setEntries((prev) => [
      ...prev,
      { name: nextServerName(prev), server: { ...EMPTY_SERVER, args: [], env: {} } },
    ]);
    setExpanded((prev) => new Set(prev).add(newIndex));
    scheduleSave();
  }

  function removeServer(index: number) {
    setEntries((prev) => prev.filter((_, i) => i !== index));
    setExpanded((prev) => {
      const next = new Set<number>();
      for (const i of prev) {
        if (i < index) next.add(i);
        else if (i > index) next.add(i - 1);
      }
      return next;
    });
    scheduleSave();
  }

  const pendingDelete = pendingDeleteIndex !== null ? entries[pendingDeleteIndex] : undefined;

  /** 表单 → JSON：先把未落盘的表单编辑保存出去，再序列化当前条目。 */
  function switchToJson() {
    if (mode === "json") return;
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    void saveNow();
    setJsonText(serializeConfig(toConfig(entriesRef.current)));
    setJsonError(null);
    setMode("json");
  }

  /** JSON → 表单：JSON 无效时留在原模式并报错，避免带着歧义状态切换。 */
  function switchToForm() {
    if (mode === "form") return;
    const parsed = parseConfigText(jsonText);
    if (!parsed.config) {
      setJsonError(parsed.error);
      return;
    }
    setEntries(toEntries(parsed.config));
    setJsonError(null);
    setMode("form");
    scheduleSave();
  }

  function handleJsonChange(text: string) {
    setJsonText(text);
    const parsed = parseConfigText(text);
    if (parsed.config) {
      setJsonError(null);
      setEntries(toEntries(parsed.config));
      scheduleSave();
    } else {
      // 无效 JSON 不进条目、不触发保存：上一份有效配置仍然生效。
      setJsonError(parsed.error);
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    }
  }

  /** Tab 插入两个空格而不是移动焦点，方便在编辑器内调整缩进。 */
  function handleJsonKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== "Tab") return;
    event.preventDefault();
    const target = event.currentTarget;
    target.setRangeText("  ", target.selectionStart, target.selectionEnd, "end");
    handleJsonChange(target.value);
  }

  async function refreshForMode() {
    const config = await loadConfig();
    if (config && mode === "json") {
      setJsonText(serializeConfig(config));
      setJsonError(null);
    }
  }

  return (
    <div className="ai-set-page">
      <Section
        id="mcp-servers"
        title="MCP 服务器"
        description="全局 MCP 注册表：这里的服务器对所有聊天会话与项目生效，配置保存在应用数据库，跟应用生命周期相同。项目可在自身 .jkcodingagent/mcp.json 中定义同名服务器覆盖全局条目。全局服务器没有项目语境，cwd 必须使用绝对路径。表单模式逐条编辑，JSON 模式直接编辑整份配置，均自动保存。"
      >
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center gap-2">
            <div className="ai-set-segment">
              <button
                type="button"
                className={cn("ai-set-segment-button", mode === "form" && "is-active")}
                onClick={switchToForm}
              >
                表单
              </button>
              <button
                type="button"
                className={cn("ai-set-segment-button", mode === "json" && "is-active")}
                onClick={switchToJson}
              >
                JSON
              </button>
            </div>
            <div className="flex-1" />
            <button
              type="button"
              className="ai-set-ghost-button"
              onClick={() => void refreshForMode()}
              disabled={loading}
            >
              <RefreshCw size={16} strokeWidth={1.5} />
              刷新
            </button>
            {mode === "form" && (
              <button type="button" className="ai-set-ghost-button" onClick={addServer}>
                <Plus size={16} strokeWidth={1.5} />
                添加服务器
              </button>
            )}
          </div>

          {loadError && <p className="ai-set-field-error">{loadError}</p>}
          {saveError && <p className="ai-set-field-error">{saveError}</p>}

          {loading ? (
            <div className="ai-settings-empty">加载中...</div>
          ) : mode === "json" ? (
            <div className="flex flex-col gap-2">
              <textarea
                className="ai-settings-textarea ai-set-json-editor font-mono"
                value={jsonText}
                spellCheck={false}
                placeholder={'{\n  "mcpServers": {\n    "my-server": {\n      "transport": "stdio",\n      "command": "npx",\n      "args": ["-y", "some-mcp-server"]\n    }\n  }\n}'}
                onChange={(event) => handleJsonChange(event.target.value)}
                onKeyDown={handleJsonKeyDown}
              />
              {jsonError ? (
                <p className="ai-set-field-error">{jsonError}</p>
              ) : (
                <p className="ai-settings-hint">
                  形状与项目级 .jkcodingagent/mcp.json 相同；JSON 有效时自动保存，无效时不会保存（上一份有效配置仍然生效）。
                </p>
              )}
            </div>
          ) : entries.length === 0 ? (
            <EmptyState
              icon={Plug}
              title="还没有全局 MCP 服务器"
              actionLabel="添加服务器"
              onAction={addServer}
            />
          ) : (
            <div className="flex flex-col gap-2">
              {entries.map((entry, index) => (
                <McpServerCard
                  key={`${entry.name}-${index}`}
                  entry={entry}
                  expanded={expanded.has(index)}
                  duplicateName={
                    entries.filter((other) => other.name.trim() === entry.name.trim()).length > 1
                  }
                  onToggleExpand={() => toggleExpand(index)}
                  onUpdate={(updater) => updateEntry(index, updater)}
                  onRemove={() => setPendingDeleteIndex(index)}
                />
              ))}
            </div>
          )}
        </div>
      </Section>

      <ConfirmDialog
        open={pendingDeleteIndex !== null}
        title="删除 MCP 服务器"
        description={
          pendingDelete
            ? `删除后所有项目与聊天将不再加载全局服务器「${pendingDelete.name}」。项目内同名覆盖不受影响。`
            : ""
        }
        confirmLabel="删除"
        onConfirm={() => {
          if (pendingDeleteIndex !== null) removeServer(pendingDeleteIndex);
          setPendingDeleteIndex(null);
        }}
        onCancel={() => setPendingDeleteIndex(null)}
      />
    </div>
  );
}

function McpServerCard({
  entry,
  expanded,
  duplicateName,
  onToggleExpand,
  onUpdate,
  onRemove,
}: {
  entry: McpEntry;
  expanded: boolean;
  duplicateName: boolean;
  onToggleExpand: () => void;
  onUpdate: (updater: (entry: McpEntry) => McpEntry) => void;
  onRemove: () => void;
}) {
  const { name, server } = entry;
  const transport = (server.transport ?? "stdio") as TransportKind;
  const updateServer = (updater: (server: McpServerConfig) => McpServerConfig) =>
    onUpdate((prev) => ({ ...prev, server: updater(prev.server) }));

  return (
    <div className="ai-set-server-card">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <input
            type="checkbox"
            checked={server.enabled ?? true}
            title={server.enabled ?? true ? "已启用" : "已停用"}
            onChange={(event) => updateServer((draft) => ({ ...draft, enabled: event.target.checked }))}
          />
          <button type="button" className="ai-set-server-title-btn" onClick={onToggleExpand}>
            <ChevronDown
              size={14}
              strokeWidth={1.5}
              className={cn("transition-transform", expanded && "rotate-180")}
            />
            <span className="truncate font-mono text-[13px]">{name || "（未命名）"}</span>
          </button>
          <span className="ai-set-chip">{TRANSPORT_LABELS[transport] ?? transport}</span>
        </div>
        <div className="flex flex-shrink-0 items-center gap-2">
          <button
            type="button"
            className="ai-set-ghost-button"
            title="删除服务器"
            onClick={onRemove}
          >
            <Trash2 size={16} strokeWidth={1.5} />
          </button>
        </div>
      </div>

      {duplicateName && (
        <p className="ai-set-field-error">服务器名称重复：同名条目保存时只会保留最后一个。</p>
      )}

      {expanded && (
        <div className="flex flex-col gap-3 border-t pt-3">
          <div className="ai-set-field">
            <FieldLabel label="名称" tip="工具将暴露为 mcp__<名称>__<工具名>；项目可用同名服务器覆盖此条目。" />
            <input
              className="ai-settings-input font-mono"
              value={name}
              spellCheck={false}
              onChange={(event) =>
                onUpdate((prev) => ({ ...prev, name: event.target.value }))
              }
            />
          </div>

          <div className="ai-set-field">
            <FieldLabel label="传输方式" tip="本地进程通过 stdin/stdout 通信；HTTP 与 Unix socket 适用于常驻服务。" />
            <select
              className="ai-settings-input"
              value={transport}
              onChange={(event) =>
                updateServer((draft) => ({
                  ...draft,
                  transport: event.target.value as TransportKind,
                }))
              }
            >
              {Object.entries(TRANSPORT_LABELS).map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
          </div>

          {transport === "stdio" && (
            <>
              <div className="ai-set-field">
                <FieldLabel label="启动命令" />
                <input
                  className="ai-settings-input font-mono"
                  value={server.command ?? ""}
                  spellCheck={false}
                  placeholder="例如 npx"
                  onChange={(event) =>
                    updateServer((draft) => ({ ...draft, command: event.target.value }))
                  }
                />
              </div>
              <div className="ai-set-field">
                <FieldLabel label="参数" tip="每行一个参数，按顺序传递给启动命令。" />
                <textarea
                  className="ai-settings-textarea font-mono"
                  rows={3}
                  spellCheck={false}
                  value={(server.args ?? []).join("\n")}
                  placeholder={"-y\nsome-mcp-server"}
                  onChange={(event) =>
                    updateServer((draft) => ({ ...draft, args: parseLines(event.target.value) }))
                  }
                />
              </div>
              <div className="ai-set-field">
                <FieldLabel label="环境变量" tip="每行一条 KEY=VALUE。" />
                <textarea
                  className="ai-settings-textarea font-mono"
                  rows={2}
                  spellCheck={false}
                  value={Object.entries(server.env ?? {}).map(([k, v]) => `${k}=${v}`).join("\n")}
                  placeholder={"API_TOKEN=xxx"}
                  onChange={(event) =>
                    updateServer((draft) => ({
                      ...draft,
                      env: parseKeyValueLines(event.target.value, /^([^=]+)=/),
                    }))
                  }
                />
              </div>
              <div className="ai-set-field">
                <FieldLabel label="工作目录（可选）" tip="全局服务器必须使用绝对路径；相对路径仅项目级 mcp.json 可用。" />
                <input
                  className="ai-settings-input font-mono"
                  value={server.cwd ?? ""}
                  spellCheck={false}
                  onChange={(event) =>
                    updateServer((draft) => ({ ...draft, cwd: event.target.value }))
                  }
                />
              </div>
            </>
          )}

          {transport === "streamable_http" && (
            <div className="ai-set-field">
              <FieldLabel label="服务地址" />
              <input
                className="ai-settings-input font-mono"
                value={server.url ?? ""}
                spellCheck={false}
                placeholder="http://127.0.0.1:3331/mcp"
                onChange={(event) =>
                  updateServer((draft) => ({ ...draft, url: event.target.value }))
                }
              />
            </div>
          )}

          {transport === "unix_socket_http" && (
            <>
              <div className="ai-set-field">
                <FieldLabel label="Socket 路径" />
                <input
                  className="ai-settings-input font-mono"
                  value={server.socketPath ?? ""}
                  spellCheck={false}
                  placeholder="/tmp/mcp.sock"
                  onChange={(event) =>
                    updateServer((draft) => ({ ...draft, socketPath: event.target.value }))
                  }
                />
              </div>
              <div className="ai-set-field">
                <FieldLabel label="HTTP 基础地址（可选）" tip="部分实现需要显式给出基础 URL。" />
                <input
                  className="ai-settings-input font-mono"
                  value={server.url ?? ""}
                  spellCheck={false}
                  onChange={(event) =>
                    updateServer((draft) => ({ ...draft, url: event.target.value }))
                  }
                />
              </div>
            </>
          )}

          {transport !== "stdio" && (
            <div className="ai-set-field">
              <FieldLabel label="请求头" tip="每行一条 Header: Value，常用于鉴权。" />
              <textarea
                className="ai-settings-textarea font-mono"
                rows={2}
                spellCheck={false}
                value={Object.entries(server.headers ?? {}).map(([k, v]) => `${k}: ${v}`).join("\n")}
                placeholder={"Authorization: Bearer xxx"}
                onChange={(event) =>
                  updateServer((draft) => ({
                    ...draft,
                    headers: parseKeyValueLines(event.target.value, /^([^:]+):\s*/),
                  }))
                }
              />
            </div>
          )}

          <div className="ai-set-field">
            <FieldLabel label="启动超时（秒，可选）" tip="默认 30 秒。" />
            <input
              className="ai-settings-input"
              type="number"
              min={1}
              max={300}
              value={server.startupTimeoutSeconds ?? ""}
              onChange={(event) =>
                updateServer((draft) => ({
                  ...draft,
                  startupTimeoutSeconds: event.target.value
                    ? Number(event.target.value)
                    : undefined,
                }))
              }
            />
          </div>
        </div>
      )}
    </div>
  );
}
