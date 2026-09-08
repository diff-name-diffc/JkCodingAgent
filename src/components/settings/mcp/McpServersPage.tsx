import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Plug, Plus, RefreshCw } from "lucide-react";
import type { McpConfig } from "../../../types";
import { cn } from "../../../lib/cn";
import { ConfirmDialog } from "../ConfirmDialog";
import { EmptyState } from "../EmptyState";
import { Section } from "../Section";
import { toast } from "../toast";
import { publishSaveSource, registerSaveSource } from "../save-sources";
import { McpServerCard } from "./McpServerCard";
import {
  AUTOSAVE_DELAY_MS,
  EMPTY_SERVER,
  nextServerName,
  parseConfigText,
  serializeConfig,
  toConfig,
  toEntries,
  type EditorMode,
  type McpEntry,
} from "./mcp-config";

/**
 * 设置弹窗的「MCP 服务器」页：全局 MCP 注册表编辑器。
 * 全局服务器对所有项目与聊天生效；项目可在自身 mcp.json 中定义同名服务器覆盖。
 * 支持表单与 JSON 两种编辑模式，共享同一份条目状态与自动保存管线。
 *
 * UI-22a：纯逻辑（条目/配置转换、解析、命名）抽至 `mcp-config.ts`（含单测），
 * 服务器条目卡抽至 `McpServerCard.tsx`，本文件仅保留页面状态与保存管线。
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

  const saveNow = useCallback(async () => {
    if (savingRef.current) return;
    savingRef.current = true;
    publishSaveSource("mcp-servers", {
      mode: "auto",
      dirty: false,
      saving: true,
      hasError: false,
    });
    try {
      const saved = await invoke<McpConfig>("mcp_global_config_save", {
        config: toConfig(entriesRef.current),
      });
      setEntries(toEntries(saved));
      setSaveError(null);
      publishSaveSource("mcp-servers", {
        mode: "auto",
        dirty: false,
        saving: false,
        hasError: false,
      });
    } catch (err) {
      setSaveError(String(err));
      publishSaveSource("mcp-servers", {
        mode: "auto",
        dirty: false,
        saving: false,
        hasError: true,
      });
      toast.error(`保存失败：${String(err)}`);
    } finally {
      savingRef.current = false;
    }
  }, []);

  const scheduleSave = useCallback(() => {
    if (timerRef.current) clearTimeout(timerRef.current);
    publishSaveSource("mcp-servers", {
      mode: "auto",
      dirty: true,
      saving: false,
      hasError: false,
    });
    timerRef.current = setTimeout(() => {
      timerRef.current = null;
      void saveNow();
    }, AUTOSAVE_DELAY_MS);
  }, [saveNow]);

  /** 立即落盘：清 debounce timer 后保存（注册表 flush / 表单→JSON 切换 / 卸载共用）。 */
  const flushPending = useCallback(() => {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    return saveNow();
  }, [saveNow]);

  useEffect(() => {
    loadConfig();
    const unregister = registerSaveSource("mcp-servers", flushPending);
    return () => {
      unregister();
      // flush-then-clear（UI-21 遗留）：卸载（切导航页）前把 debounce 窗口内的
      // 待保存编辑落盘——saveNow 经 entriesRef 读最新值，组件销毁后的 setState
      // 为 no-op，不影响落库。此前 cleanup 只 clearTimeout，会丢 400ms 内编辑。
      if (timerRef.current) void flushPending();
    };
  }, [loadConfig, flushPending]);

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
    void flushPending();
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
