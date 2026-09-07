import { ChevronDown, Trash2 } from "lucide-react";
import type { McpServerConfig } from "../../../types";
import { cn } from "../../../lib/cn";
import { FieldLabel } from "../FieldLabel";
import {
  parseKeyValueLines,
  parseLines,
  TRANSPORT_LABELS,
  type McpEntry,
  type TransportKind,
} from "./mcp-config";

/**
 * 单个全局 MCP 服务器条目卡（UI-22a 从 McpServersPage 抽离）。
 * 折叠态显示启用开关/名称/传输方式/删除；展开态按传输方式渲染对应字段，
 * 所有编辑经 onUpdate 回流页面状态并触发自动保存。
 */
export function McpServerCard({
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
          <span>{TRANSPORT_LABELS[transport] ?? transport}</span>
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
