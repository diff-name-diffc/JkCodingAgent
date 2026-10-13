import { useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import type { ModelCategory, SubAgentConfig } from "../../../types";
import { BROWSER_AGENT_ID } from "../../../lib/sub-agent-runtime";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../../ui/select";
import { useAhaSettings } from "../use-aha-settings";
import { entryLabel } from "../providers/model-library";
import {
  findMatchedLibraryEntry,
  pickableCategoryOptions,
  pickableEntries,
} from "./sub-agent-model-picker";
import { MAX_ITERATIONS, MIN_OUTPUT_TOKENS, MAX_OUTPUT_TOKENS } from "./sub-agent-validation";

interface Props {
  draft: SubAgentConfig;
  setDraft: Dispatch<SetStateAction<SubAgentConfig>>;
}

export function SubAgentRuntimeEditor({ draft, setDraft }: Props) {
  const { settings } = useAhaSettings();
  const modelLibrary = settings?.modelLibrary ?? [];
  // 模型选择器第一步（分类）：初值取当前配置命中的库条目分类，无命中则「对话模型」。
  const [pickerCategory, setPickerCategory] = useState<ModelCategory>(
    () => findMatchedLibraryEntry(modelLibrary, draft.modelConfig)?.category ?? "text",
  );

  const categoryEntries = pickableEntries(modelLibrary, pickerCategory);
  const matchedEntry = findMatchedLibraryEntry(modelLibrary, draft.modelConfig);
  const modelSelectValue =
    matchedEntry && matchedEntry.category === pickerCategory ? matchedEntry.id : "";

  function handlePickCategory(value: string) {
    setPickerCategory(value as ModelCategory);
  }

  function handlePickModel(entryId: string) {
    const picked = categoryEntries.find((entry) => entry.id === entryId);
    if (!picked) return;
    setDraft((d) => ({
      ...d,
      modelConfig: {
        ...d.modelConfig,
        inheritFromParent: false,
        apiBase: picked.url,
        apiKey: picked.apiKey,
        modelName: picked.model,
      },
    }));
  }

  return (
    <>
      <div className="ai-aha-section">
        <div className="ai-aha-section-title">模型配置</div>
        <label className="ai-subagent-choice-row">
          <input
            type="radio"
            name="modelMode"
            checked={draft.modelConfig.inheritFromParent}
            onChange={() =>
              setDraft((d) => ({
                ...d,
                modelConfig: { ...d.modelConfig, inheritFromParent: true },
              }))
            }
          />
          <span>继承主 Agent 配置</span>
        </label>
        <label className="ai-subagent-choice-row">
          <input
            type="radio"
            name="modelMode"
            checked={!draft.modelConfig.inheritFromParent}
            onChange={() =>
              setDraft((d) => ({
                ...d,
                modelConfig: { ...d.modelConfig, inheritFromParent: false },
              }))
            }
          />
          <span>自定义配置</span>
        </label>
        {!draft.modelConfig.inheritFromParent && (
          <>
            <div className="ai-settings-field-stack">
              <label className="ai-settings-field-label">选择已配置的模型</label>
              <div className="ai-subagent-model-picker">
                <Select value={pickerCategory} onValueChange={handlePickCategory}>
                  <SelectTrigger aria-label="模型分类">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {pickableCategoryOptions().map((option) => (
                      <SelectItem key={option.category} value={option.category}>
                        {option.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {categoryEntries.length === 0 ? (
                  <span className="ai-settings-hint">
                    该分类暂无已配置的启用模型，请前往「模型服务」添加，或在下方手动填写。
                  </span>
                ) : (
                  <Select value={modelSelectValue} onValueChange={handlePickModel}>
                    <SelectTrigger aria-label="模型">
                      <SelectValue placeholder="选择模型…" />
                    </SelectTrigger>
                    <SelectContent>
                      {categoryEntries.map((entry) => (
                        <SelectItem key={entry.id} value={entry.id}>
                          <span className="ai-subagent-model-option">
                            <span className="ai-subagent-model-option-name">
                              {entryLabel(entry)}
                            </span>
                            {entry.alias?.trim() &&
                            entry.model.trim() &&
                            entry.alias.trim() !== entry.model.trim() ? (
                              <span className="ai-subagent-model-option-model">{entry.model}</span>
                            ) : null}
                          </span>
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              </div>
              <span className="ai-settings-hint">
                选中后自动回填下方 API Base / API Key / 模型名称，可再手动微调。
              </span>
            </div>
            <div className="ai-settings-field-stack">
              <label className="ai-settings-field-label">API Base</label>
              <input
                className="ai-settings-input"
                value={draft.modelConfig.apiBase ?? ""}
                placeholder="https://api.openai.com/v1"
                onChange={(e) =>
                  setDraft((d) => ({
                    ...d,
                    modelConfig: { ...d.modelConfig, apiBase: e.target.value },
                  }))
                }
              />
            </div>
            <div className="ai-settings-field-stack">
              <label className="ai-settings-field-label">API Key</label>
              <input
                className="ai-settings-input"
                type="password"
                value={draft.modelConfig.apiKey ?? ""}
                onChange={(e) =>
                  setDraft((d) => ({
                    ...d,
                    modelConfig: { ...d.modelConfig, apiKey: e.target.value },
                  }))
                }
              />
            </div>
            <div className="ai-settings-field-stack">
              <label className="ai-settings-field-label">模型名称</label>
              <input
                className="ai-settings-input"
                value={draft.modelConfig.modelName ?? ""}
                placeholder="为空时使用主 Agent 同类型模型"
                onChange={(e) =>
                  setDraft((d) => ({
                    ...d,
                    modelConfig: { ...d.modelConfig, modelName: e.target.value },
                  }))
                }
              />
            </div>
          </>
        )}
      </div>

      {draft.agentId === BROWSER_AGENT_ID && (
        <div className="ai-settings-field-stack">
          <label className="ai-subagent-choice-row">
            <input
              type="checkbox"
              checked={draft.browserUseVision ?? false}
              onChange={(e) => setDraft((d) => ({ ...d, browserUseVision: e.target.checked }))}
            />
            <span>使用截图视觉</span>
          </label>
          <span className="ai-settings-hint">
            默认仅使用 DOM 文本。开启后会向所选模型发送页面截图，需要模型支持图像输入。
          </span>
        </div>
      )}

      <div className="ai-subagent-runtime-grid">
        <div className="ai-settings-field-stack">
          <label className="ai-settings-field-label">
            {draft.agentId === BROWSER_AGENT_ID ? "最大执行步数" : "最大迭代轮次"} (1-
            {MAX_ITERATIONS})
          </label>
          <input
            className="ai-settings-input"
            type="number"
            min={1}
            max={MAX_ITERATIONS}
            value={draft.maxIterations}
            onChange={(e) => setDraft((d) => ({ ...d, maxIterations: Number(e.target.value) }))}
          />
        </div>
        <div className="ai-settings-field-stack">
          <label className="ai-settings-field-label">
            单次响应最大输出 Token ({MIN_OUTPUT_TOKENS}-{MAX_OUTPUT_TOKENS})
          </label>
          <input
            className="ai-settings-input"
            type="number"
            min={MIN_OUTPUT_TOKENS}
            max={MAX_OUTPUT_TOKENS}
            value={draft.maxOutputTokens}
            onChange={(e) => setDraft((d) => ({ ...d, maxOutputTokens: Number(e.target.value) }))}
          />
          <span className="ai-settings-hint">
            单次模型响应的输出上限，不代表上下文窗口或整个任务的 Token
            预算。实际值必须不超过所选模型服务允许的最大输出值。
          </span>
        </div>
        <div className="ai-settings-field-stack">
          <label className="ai-settings-field-label">Temperature (0-2)</label>
          <input
            className="ai-settings-input"
            type="number"
            min={0}
            max={2}
            step={0.1}
            value={draft.temperature}
            onChange={(e) => {
              // 清空输入不等于想要 0（Number("") === 0），仅在可解析时写回。
              const next = e.target.value.trim();
              if (next !== "" && Number.isFinite(Number(next))) {
                setDraft((d) => ({ ...d, temperature: Number(next) }));
              }
            }}
          />
        </div>
        <div className="ai-settings-field-stack">
          <label className="ai-settings-field-label">超时时间 (1-3600 秒)</label>
          <input
            className="ai-settings-input"
            type="number"
            min={1}
            max={3600}
            value={draft.timeoutSecs}
            onChange={(e) => setDraft((d) => ({ ...d, timeoutSecs: Number(e.target.value) }))}
          />
        </div>
      </div>
    </>
  );
}
