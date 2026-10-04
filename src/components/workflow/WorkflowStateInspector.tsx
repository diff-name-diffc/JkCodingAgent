import { useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import type { WorkflowDefinition, WorkflowPlanRecord } from "../../types";
import { cn } from "../../lib/cn";
import { parseWorkflowState, previewStateValue } from "./workflow-utils";

/** 底部可折叠的共享 state 检查器：key + 描述 + 实时值预览。 */
export function WorkflowStateInspector({
  plan,
  definition,
  open,
  onToggle,
}: {
  plan: WorkflowPlanRecord | null;
  definition: WorkflowDefinition | null;
  open: boolean;
  onToggle: () => void;
}) {
  const [expandedKey, setExpandedKey] = useState<string | null>(null);
  const state = parseWorkflowState(plan);

  // stateKeys 声明的 key 在前；state 里多出的 key（未声明）排在后面。
  const declaredKeys = definition?.stateKeys ?? [];
  const extraKeys = Object.keys(state).filter(
    (key) => !declaredKeys.some((entry) => entry.key === key),
  );
  const rows = [
    ...declaredKeys.map((entry) => ({
      key: entry.key,
      description: entry.description,
      declared: true,
    })),
    ...extraKeys.map((key) => ({ key, description: "", declared: false })),
  ];

  return (
    <section className="ai-workflow-state">
      <button
        type="button"
        className="ai-workflow-state-header"
        onClick={onToggle}
        aria-expanded={open}
      >
        <span className="ai-workflow-state-title">共享状态</span>
        <span className="ai-workflow-state-count">{rows.length} 个键</span>
        {open ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronUp className="h-3.5 w-3.5" />}
      </button>
      {open && (
        <div className="ai-workflow-state-body">
          {rows.length === 0 && (
            <div className="ai-workflow-state-empty">该工作流未声明共享状态键。</div>
          )}
          {rows.map((row) => {
            const hasValue = Object.prototype.hasOwnProperty.call(state, row.key);
            const expanded = expandedKey === row.key;
            const value = hasValue ? state[row.key] : undefined;
            const fullText =
              value === undefined
                ? ""
                : typeof value === "string"
                  ? value
                  : (JSON.stringify(value, null, 2) ?? String(value));
            return (
              <div key={row.key} className="ai-workflow-state-row">
                <button
                  type="button"
                  className="ai-workflow-state-row-head"
                  onClick={() => hasValue && setExpandedKey(expanded ? null : row.key)}
                  aria-expanded={expanded}
                  disabled={!hasValue}
                >
                  <span
                    className={cn(
                      "ai-workflow-state-key",
                      !row.declared && "ai-workflow-state-key--extra",
                    )}
                  >
                    {row.key}
                  </span>
                  {row.description && (
                    <span className="ai-workflow-state-desc">{row.description}</span>
                  )}
                  <span className="ai-workflow-state-value">
                    {hasValue ? (expanded ? "" : previewStateValue(value)) : "（尚未写入）"}
                  </span>
                </button>
                {expanded && hasValue && <pre className="ai-workflow-state-full">{fullText}</pre>}
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}
