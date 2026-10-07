import { useState } from "react";
import { useAhaSettings } from "../use-aha-settings";
import { Section } from "../Section";
import { FieldLabel } from "../FieldLabel";
import { parseBoundedNumberInput } from "../rag/rag-config";
import { toast } from "../../Toast";
import {
  patchToolTimeouts,
  timeoutFieldValue,
  TOOL_TIMEOUT_FIELD_DEFS,
  type ToolTimeoutFieldDef,
} from "./tool-timeouts";

/**
 * 「工具超时」节：白名单工具的默认超时（图片生成 / 编辑 / 下载）。
 * 未配置 = 策略表默认；Agent 可在调用参数 timeout_secs 中声明更长预算
 * （上限与本节一致）。变更走自动保存管线（fieldId 定位内联错误）。
 */
export function ToolTimeoutsSection() {
  const store = useAhaSettings();
  if (store.loading || !store.settings) {
    return null;
  }
  return (
    <Section
      title="工具超时"
      description="长耗时工具的默认超时预算（秒）；Agent 也可在调用时声明更长，上限一致。未配置 = 使用内置默认。"
    >
      {TOOL_TIMEOUT_FIELD_DEFS.map((def) => (
        <ToolTimeoutField key={def.key} def={def} />
      ))}
    </Section>
  );
}

/** 单个超时字段：string 草稿 + 失焦提交（空 = 清除、越界提示并还原、
 * badInput 同样还原），模式与 ModelEntryCard 的容量字段一致。 */
function ToolTimeoutField({ def }: { def: ToolTimeoutFieldDef }) {
  const store = useAhaSettings();
  const settings = store.settings;
  const current = settings ? timeoutFieldValue(settings, def.key) : undefined;
  const [draft, setDraft] = useState(current != null ? String(current) : "");
  const fieldId = `tool-timeout:${def.key}`;
  const fieldError = store.saveError?.fieldId === fieldId ? store.saveError.message : null;

  function commit(badInput: boolean) {
    if (!settings) return;
    const trimmed = draft.trim();
    if (trimmed === "" && badInput) {
      toast.error(`${def.label}超时需为 ${def.min}–${def.max} 之间的数值`);
      setDraft(current != null ? String(current) : "");
      return;
    }
    if (trimmed === "") {
      setDraft("");
      if (current !== undefined) {
        store.updateSettings((prev) => patchToolTimeouts(prev, def.key, undefined), fieldId);
      }
      return;
    }
    const parsed = parseBoundedNumberInput(trimmed, def.min, def.max);
    if (parsed === null) {
      toast.error(`${def.label}超时需为 ${def.min}–${def.max} 之间的数值`);
      setDraft(current != null ? String(current) : "");
      return;
    }
    const next = Math.floor(parsed);
    setDraft(String(next));
    if (next !== current) {
      store.updateSettings((prev) => patchToolTimeouts(prev, def.key, next), fieldId);
    }
  }

  return (
    <div className="ai-set-field">
      <FieldLabel label={`${def.label}超时（秒）`} tip={def.tip} />
      <input
        className="ai-settings-input"
        type="number"
        min={def.min}
        max={def.max}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={(e) => commit(e.currentTarget.validity.badInput)}
        placeholder={`留空 = 默认 ${def.defaultSecs} 秒`}
        spellCheck={false}
      />
      {fieldError && <div className="ai-set-field-error">{fieldError}</div>}
    </div>
  );
}
