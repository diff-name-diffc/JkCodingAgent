import { useState } from "react";
import { useAhaSettings } from "../use-aha-settings";
import { Section } from "../Section";
import { FieldLabel } from "../FieldLabel";
import { parseBoundedNumberInput } from "../rag/rag-config";
import { toast } from "../../Toast";
import {
  DEFAULT_TOOL_ITERATIONS,
  TOOL_ITERATIONS_RANGE,
  maxToolIterationsValue,
  patchMaxToolIterations,
} from "./tool-iterations";

/**
 * 「工具循环上限」节：主对话循环单次 run 的最大工具迭代轮数。
 * 未配置 = 内置默认；对普通聊天 / 项目编排 / 架构助手统一生效（子智能体
 * 轮数按子智能体自身配置，不受此项控制）。变更走自动保存管线。
 */
export function ToolIterationsSection() {
  const store = useAhaSettings();
  if (store.loading || !store.settings) {
    return null;
  }
  return (
    <Section
      title="工具循环上限"
      description="主对话单次运行允许的最大工具迭代轮数；轮数到顶会终止本轮。未配置 = 使用内置默认。"
    >
      <IterationsField />
    </Section>
  );
}

/** 轮数字段：string 草稿 + 失焦提交（空 = 清除、越界提示并还原、badInput 同样
 * 还原），模式与 ToolTimeoutsSection 的超时字段一致。 */
function IterationsField() {
  const store = useAhaSettings();
  const settings = store.settings;
  const current = settings ? maxToolIterationsValue(settings) : undefined;
  const [draft, setDraft] = useState(current != null ? String(current) : "");
  const fieldId = "tool-iterations:max";
  const fieldError = store.saveError?.fieldId === fieldId ? store.saveError.message : null;

  function commit(badInput: boolean) {
    if (!settings) return;
    const trimmed = draft.trim();
    if (trimmed === "" && badInput) {
      toast.error(
        `工具循环上限需为 ${TOOL_ITERATIONS_RANGE.min}–${TOOL_ITERATIONS_RANGE.max} 之间的数值`,
      );
      setDraft(current != null ? String(current) : "");
      return;
    }
    if (trimmed === "") {
      setDraft("");
      if (current !== undefined) {
        store.updateSettings((prev) => patchMaxToolIterations(prev, undefined), fieldId);
      }
      return;
    }
    const parsed = parseBoundedNumberInput(
      trimmed,
      TOOL_ITERATIONS_RANGE.min,
      TOOL_ITERATIONS_RANGE.max,
    );
    if (parsed === null) {
      toast.error(
        `工具循环上限需为 ${TOOL_ITERATIONS_RANGE.min}–${TOOL_ITERATIONS_RANGE.max} 之间的数值`,
      );
      setDraft(current != null ? String(current) : "");
      return;
    }
    const next = Math.floor(parsed);
    setDraft(String(next));
    if (next !== current) {
      store.updateSettings((prev) => patchMaxToolIterations(prev, next), fieldId);
    }
  }

  return (
    <div className="ai-set-field">
      <FieldLabel
        label="工具迭代轮数上限（轮）"
        tip="主对话循环（聊天 / 项目编排 / 架构助手）单次运行的最大工具调用轮数。长任务可调大；调小可及早掐断陷入循环的模型。子智能体的轮数在其自身配置中单独设置。"
      />
      <input
        className="ai-settings-input"
        type="number"
        min={TOOL_ITERATIONS_RANGE.min}
        max={TOOL_ITERATIONS_RANGE.max}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={(e) => commit(e.currentTarget.validity.badInput)}
        placeholder={`留空 = 默认 ${DEFAULT_TOOL_ITERATIONS} 轮`}
        spellCheck={false}
      />
      {fieldError && <div className="ai-set-field-error">{fieldError}</div>}
    </div>
  );
}
