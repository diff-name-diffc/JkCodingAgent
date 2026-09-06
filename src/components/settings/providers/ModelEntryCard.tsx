import { useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, Pencil, RefreshCw, Search, Trash2 } from "lucide-react";
import { useAhaSettings } from "../use-aha-settings";
import { ApiKeyInput } from "../ApiKeyInput";
import { ConfirmDialog } from "../ConfirmDialog";
import { FieldLabel } from "../FieldLabel";
import { StatusBadge } from "../StatusBadge";
import { TestButton } from "../TestButton";
import { toast } from "../toast";
import { Tooltip, TooltipContent, TooltipTrigger } from "../../ui/tooltip";
import { isImeComposing } from "../../../utils";
import { parseBoundedNumberInput } from "../../app-settings/rag/rag-config";
import type { ModelLibraryEntry } from "../../../types";
import {
  entryLabel,
  ENTRY_CONTEXT_WINDOW_RANGE,
  ENTRY_MAX_TOKENS_RANGE,
  type ModelCategoryDef,
} from "./model-library";
import { ProviderIcon } from "./ProviderIcon";

/**
 * 模型库条目卡片：品牌图标 + 别名 + 状态徽标 + 启用开关 + 删除，
 * 展开后编辑 model / URL / API Key，并可「获取模型」「测试连接」。
 * 「获取模型」的结果以可搜索下拉形式展示在模型名称输入框下方。
 * 父组件必须以 key={entry.id} 渲染。
 */
export function ModelEntryCard({
  entry,
  def,
  expanded,
  usageTitles,
  testStatus,
  onToggleExpand,
  onPatch,
  onRemove,
  onTestResult,
}: {
  entry: ModelLibraryEntry;
  def: ModelCategoryDef;
  expanded: boolean;
  /** 引用该条目的用途标题（删除确认时展示）。 */
  usageTitles: string[];
  testStatus: "ok" | "failed" | "untested";
  onToggleExpand: () => void;
  onPatch: (patch: Partial<Omit<ModelLibraryEntry, "id" | "category">>) => void;
  onRemove: () => void;
  onTestResult: (status: "ok" | "failed") => void;
}) {
  const store = useAhaSettings();
  const [model, setModel] = useState(entry.model);
  const [url, setUrl] = useState(entry.url);
  const [apiKey, setApiKey] = useState(entry.apiKey);
  // 容量草稿用字符串承载（允许中间态为空/非法），失焦时解析提交。
  const [maxTokensDraft, setMaxTokensDraft] = useState(
    entry.maxTokens != null ? String(entry.maxTokens) : "",
  );
  const [contextWindowDraft, setContextWindowDraft] = useState(
    entry.contextWindow != null ? String(entry.contextWindow) : "",
  );
  const [editingAlias, setEditingAlias] = useState(false);
  const [aliasDraft, setAliasDraft] = useState("");
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [fetchedModels, setFetchedModels] = useState<string[] | null>(null);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [modelQuery, setModelQuery] = useState("");

  const alias = entryLabel(entry);
  const enabled = entry.enabled !== false;
  const fieldId = `model:${entry.id}`;
  const fieldError = store.saveError?.fieldId === fieldId ? store.saveError.message : null;

  const filteredModels = useMemo(() => {
    if (!fetchedModels) return [];
    const q = modelQuery.trim().toLowerCase();
    return q ? fetchedModels.filter((m) => m.toLowerCase().includes(q)) : fetchedModels;
  }, [fetchedModels, modelQuery]);

  function commitField(field: "model" | "url" | "apiKey", value: string) {
    const next = value.trim();
    if (next === entry[field]) return;
    onPatch({ [field]: next });
  }

  /** 容量字段失焦提交：空 → 清除（undefined，后端序列化时整个键被丢弃）；
   * 越界/非法 → 还原草稿为已存值并提示，不提交。badInput=true 表示浏览器把
   * 非法键入（如 "abc"）规整成了空值——那是非法输入而非用户清空，同样还原，
   * 避免静默清掉已配置值。 */
  function commitCapacityField(
    field: "maxTokens" | "contextWindow",
    draft: string,
    range: { min: number; max: number },
    label: string,
    resetDraft: (value: string) => void,
    badInput: boolean,
  ) {
    const current = entry[field];
    const trimmed = draft.trim();
    if (trimmed === "" && badInput) {
      toast.error(`${label}需为 ${range.min}–${range.max} 之间的数值`);
      resetDraft(current != null ? String(current) : "");
      return;
    }
    if (trimmed === "") {
      resetDraft("");
      if (current !== undefined) {
        onPatch(field === "maxTokens" ? { maxTokens: undefined } : { contextWindow: undefined });
      }
      return;
    }
    const parsed = parseBoundedNumberInput(trimmed, range.min, range.max);
    if (parsed === null) {
      toast.error(`${label}需为 ${range.min}–${range.max} 之间的数值`);
      resetDraft(current != null ? String(current) : "");
      return;
    }
    const next = Math.floor(parsed);
    resetDraft(String(next));
    if (next !== current) {
      onPatch(field === "maxTokens" ? { maxTokens: next } : { contextWindow: next });
    }
  }

  function commitAlias() {
    setEditingAlias(false);
    const next = aliasDraft.trim();
    if (next !== (entry.alias ?? "")) onPatch({ alias: next });
  }

  async function fetchModels() {
    if (fetchedModels) {
      setFetchedModels(null);
      return;
    }
    setFetchingModels(true);
    try {
      const list = await invoke<string[]>("dispatcher_fetch_models", {
        apiBase: entry.url,
        apiKey: entry.apiKey,
      });
      if (list.length === 0) {
        toast.error("服务商未返回任何模型");
      } else {
        setModelQuery("");
        setFetchedModels(list);
      }
    } catch (error) {
      toast.error(`获取模型失败：${String(error)}`);
    } finally {
      setFetchingModels(false);
    }
  }

  function pickModel(value: string) {
    setFetchedModels(null);
    setModel(value);
    commitField("model", value);
  }

  return (
    <div className={enabled ? "ai-set-provider" : "ai-set-provider is-disabled"}>
      <div className="ai-set-provider-header">
        <button type="button" className="ai-set-provider-title" onClick={onToggleExpand}>
          <ChevronDown
            size={16}
            strokeWidth={1.5}
            className="ai-set-provider-chevron"
            style={{ transform: expanded ? "none" : "rotate(-90deg)" }}
          />
          <ProviderIcon url={entry.url} name={alias} size={20} />
          {editingAlias ? (
            <input
              autoFocus
              className="ai-settings-input ai-set-alias-input"
              value={aliasDraft}
              onChange={(e) => setAliasDraft(e.target.value)}
              onBlur={commitAlias}
              onKeyDown={(e) => {
                if (!isImeComposing(e) && e.key === "Enter") commitAlias();
                if (e.key === "Escape") setEditingAlias(false);
              }}
              onClick={(e) => e.stopPropagation()}
              spellCheck={false}
            />
          ) : (
            <span className="ai-set-provider-name">
              {alias}
              <span
                role="button"
                tabIndex={0}
                className="ai-set-alias-edit"
                aria-label="重命名模型"
                onClick={(e) => {
                  e.stopPropagation();
                  setAliasDraft(entry.alias ?? "");
                  setEditingAlias(true);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.stopPropagation();
                    setAliasDraft(entry.alias ?? "");
                    setEditingAlias(true);
                  }
                }}
              >
                <Pencil size={14} strokeWidth={1.5} />
              </span>
            </span>
          )}
          <StatusBadge status={testStatus} />
        </button>
        <div className="ai-set-provider-actions">
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                role="switch"
                aria-checked={enabled}
                aria-label={enabled ? "停用模型" : "启用模型"}
                className={enabled ? "ai-set-switch is-on" : "ai-set-switch"}
                onClick={() => onPatch({ enabled: !enabled })}
              >
                <span className="ai-set-switch-thumb" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="top">
              {enabled ? "停用后不再出现在模型用途的选项中" : "启用后可绑定到模型用途"}
            </TooltipContent>
          </Tooltip>
          <button
            type="button"
            className="ai-set-icon-button is-danger"
            onClick={() => setConfirmingDelete(true)}
            aria-label="删除模型"
            title="删除模型"
          >
            <Trash2 size={16} strokeWidth={1.5} />
          </button>
        </div>
      </div>

      {usageTitles.length > 0 && (
        <div className="ai-set-provider-usage">已用于：{usageTitles.join("、")}</div>
      )}

      {expanded && (
        <div className="ai-set-provider-body">
          <div className="ai-set-field">
            <div className="ai-set-field-row">
              <FieldLabel label="模型名称" tip="服务商接口中的模型标识，如 gpt-4o、deepseek-chat。" />
              {def.isModelListFetchable && (
                <button
                  type="button"
                  className="ai-set-ghost-button"
                  onClick={() => void fetchModels()}
                  disabled={fetchingModels || !entry.url || !entry.apiKey}
                  title={!entry.url || !entry.apiKey ? "先填写 URL 和 API Key" : undefined}
                >
                  <RefreshCw
                    size={16}
                    strokeWidth={1.5}
                    className={fetchingModels ? "animate-spin" : undefined}
                  />
                  获取模型
                </button>
              )}
            </div>
            <div className="ai-set-model-field">
              <input
                className="ai-settings-input"
                value={model}
                onChange={(e) => setModel(e.target.value)}
                onBlur={() => commitField("model", model)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") setFetchedModels(null);
                }}
                placeholder="模型名称，如 gpt-4o"
                spellCheck={false}
              />
              {fetchedModels && (
                <>
                  <div
                    className="ai-set-model-dropdown-backdrop"
                    onMouseDown={() => setFetchedModels(null)}
                  />
                  <div className="ai-set-model-dropdown">
                    <div className="ai-set-model-search">
                      <Search size={16} strokeWidth={1.5} />
                      <input
                        autoFocus
                        value={modelQuery}
                        onChange={(e) => setModelQuery(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Escape") setFetchedModels(null);
                          if (!isImeComposing(e) && e.key === "Enter" && filteredModels.length > 0) {
                            pickModel(filteredModels[0]);
                          }
                        }}
                        placeholder="搜索模型名称..."
                        spellCheck={false}
                      />
                    </div>
                    <div className="ai-set-model-list chat-scroll">
                      {filteredModels.length === 0 ? (
                        <div className="ai-set-model-list-empty">没有匹配的模型</div>
                      ) : (
                        filteredModels.map((item) => (
                          <button
                            key={item}
                            type="button"
                            className={
                              item === entry.model.trim()
                                ? "ai-set-model-option is-active"
                                : "ai-set-model-option"
                            }
                            onMouseDown={(e) => e.preventDefault()}
                            onClick={() => pickModel(item)}
                          >
                            <span className="ai-set-model-option-name">{item}</span>
                          </button>
                        ))
                      )}
                    </div>
                  </div>
                </>
              )}
            </div>
          </div>
          <div className="ai-set-field">
            <FieldLabel label="API 地址（URL）" tip="服务商的 OpenAI 兼容接口地址，通常以 /v1 结尾。" />
            <input
              className="ai-settings-input"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              onBlur={() => commitField("url", url)}
              placeholder="https://api.example.com/v1"
              spellCheck={false}
            />
          </div>
          <div className="ai-set-field">
            <FieldLabel label="API Key" tip="在服务商控制台创建的密钥，仅保存在本机。" />
            <ApiKeyInput
              value={apiKey}
              onChange={setApiKey}
              onBlur={() => commitField("apiKey", apiKey)}
            />
          </div>
          {def.hasCapacityFields && (
            <>
              <div className="ai-set-field">
                <FieldLabel
                  label="输出预算（maxTokens）"
                  tip="单次请求可见输出与思考链共享的 token 上限。留空则请求省略 max_tokens、由服务端默认预算接管（推荐）；思考模型配过小预算时思考链会耗尽预算导致空响应。"
                />
                <input
                  className="ai-settings-input"
                  type="number"
                  min={ENTRY_MAX_TOKENS_RANGE.min}
                  max={ENTRY_MAX_TOKENS_RANGE.max}
                  value={maxTokensDraft}
                  onChange={(e) => setMaxTokensDraft(e.target.value)}
                  onBlur={(e) =>
                    commitCapacityField(
                      "maxTokens",
                      maxTokensDraft,
                      ENTRY_MAX_TOKENS_RANGE,
                      "输出预算",
                      setMaxTokensDraft,
                      e.currentTarget.validity.badInput,
                    )
                  }
                  placeholder="留空 = 服务端默认预算"
                  spellCheck={false}
                />
              </div>
              <div className="ai-set-field">
                <FieldLabel
                  label="上下文窗口（contextWindow）"
                  tip="模型上下文窗口的 token 总容量，驱动上下文裁剪阈值与容量展示；留空默认 1,000,000。"
                />
                <input
                  className="ai-settings-input"
                  type="number"
                  min={ENTRY_CONTEXT_WINDOW_RANGE.min}
                  max={ENTRY_CONTEXT_WINDOW_RANGE.max}
                  value={contextWindowDraft}
                  onChange={(e) => setContextWindowDraft(e.target.value)}
                  onBlur={(e) =>
                    commitCapacityField(
                      "contextWindow",
                      contextWindowDraft,
                      ENTRY_CONTEXT_WINDOW_RANGE,
                      "上下文窗口",
                      setContextWindowDraft,
                      e.currentTarget.validity.badInput,
                    )
                  }
                  placeholder="留空 = 默认 1,000,000"
                  spellCheck={false}
                />
              </div>
            </>
          )}
          {fieldError && <div className="ai-set-field-error">{fieldError}</div>}

          <TestButton
            label="测试连接"
            disabled={!entry.url.trim() || !entry.model.trim()}
            onResult={(result) => {
              if (result) onTestResult(result.status === "success" ? "ok" : "failed");
            }}
            onTest={() =>
              invoke<string>("dispatcher_test_model", {
                kind: def.testKind,
                config: {
                  url: entry.url,
                  apiKey: entry.apiKey,
                  model: entry.model,
                  active: true,
                },
              })
            }
          />
        </div>
      )}

      <ConfirmDialog
        open={confirmingDelete}
        title={`删除「${alias}」？`}
        description={
          usageTitles.length > 0
            ? `删除后使用此模型的 ${usageTitles.length} 个用途（${usageTitles.join("、")}）将失效，需要重新绑定。`
            : "删除后该模型条目将被移除。"
        }
        onConfirm={() => {
          setConfirmingDelete(false);
          onRemove();
        }}
        onCancel={() => setConfirmingDelete(false)}
      />
    </div>
  );
}
