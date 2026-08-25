import { useCallback, useEffect, useRef, useState } from "react";
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

/**
 * 设置弹窗的「MCP 服务器」页：全局 MCP 注册表编辑器。
 * 全局服务器对所有项目与聊天生效；项目可在自身 mcp.json 中定义同名服务器覆盖。
 */
export function McpServersPage() {
  const [entries, setEntries] = useState<McpEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [pendingDeleteIndex, setPendingDeleteIndex] = useState<number | null>(null);

  const entriesRef = useRef(entries);
  entriesRef.current = entries;
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savingRef = useRef(false);

  const loadConfig = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const config = await invoke<McpConfig>("mcp_global_config_get");
      setEntries(toEntries(config));
    } catch (err) {
      setLoadError(String(err));
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

  return (
    <div className="ai-set-page">
      <Section
        id="mcp-servers"
        title="MCP 服务器"
        description="全局 MCP 注册表：这里的服务器对所有聊天会话与项目生效，配置保存在应用数据库，跟应用生命周期相同。项目可在自身 .jkcodingagent/mcp.json 中定义同名服务器覆盖全局条目。全局服务器没有项目语境，cwd 必须使用绝对路径。字段失焦或开关切换后自动保存。"
      >
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center gap-2">
            <div className="flex-1" />
            <button
              type="button"
              className="ai-set-ghost-button"
              onClick={loadConfig}
              disabled={loading}
            >
              <RefreshCw size={16} strokeWidth={1.5} />
              刷新
            </button>
            <button type="button" className="ai-set-ghost-button" onClick={addServer}>
              <Plus size={16} strokeWidth={1.5} />
              添加服务器
            </button>
          </div>

          {loadError && <p className="ai-set-field-error">{loadError}</p>}
          {saveError && <p className="ai-set-field-error">{saveError}</p>}

          {loading ? (
            <div className="ai-settings-empty">加载中...</div>
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
