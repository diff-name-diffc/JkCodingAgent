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
import type { ModelLibraryEntry } from "../../../types";
import { entryLabel, type ModelCategoryDef } from "./model-library";
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
