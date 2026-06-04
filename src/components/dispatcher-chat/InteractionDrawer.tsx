import { memo, useState } from "react";
import { ClipboardList, FileText } from "lucide-react";
import type { AgentType, ChecklistPlanState, PlanInteraction } from "../../types";
import { getSubProcessAgentLabel } from "./dispatcherChatUtils";
import { dispatcherChatStyles as styles } from "./dispatcherChatStyles";

export const InteractionDrawer = memo(function InteractionDrawer({
  checklist,
  planInteraction,
  implementingPlan,
  onAnswerPlanQuestion,
  onImplementPlan,
  onImplementPlanWithClearedContext,
  onStayInPlanMode,
}: {
  checklist: ChecklistPlanState | null;
  planInteraction: PlanInteraction | null;
  implementingPlan: boolean;
  onAnswerPlanQuestion: (answer: string) => void;
  onImplementPlan: (interaction: Extract<PlanInteraction, { kind: "ready" }>) => void;
  onImplementPlanWithClearedContext: (
    interaction: Extract<PlanInteraction, { kind: "ready" }>,
  ) => void;
  onStayInPlanMode: () => void;
}) {
  const [customAnswer, setCustomAnswer] = useState("");

  if (planInteraction?.kind === "question") {
    return (
      <div style={styles.drawer}>
        <div style={styles.drawerHeader}>
          <span style={styles.drawerTitle}>
            <ClipboardList size={14} />
            问题清单
          </span>
        </div>
        <div style={styles.drawerQuestion}>{planInteraction.question}</div>
        <div style={styles.drawerOptionGrid}>
          {planInteraction.options.map((option) => (
            <button
              key={option.id}
              type="button"
              style={styles.drawerOptionBtn}
              onClick={() =>
                onAnswerPlanQuestion(`选择：${option.label}\n说明：${option.description}`)
              }
            >
              <span style={styles.drawerOptionLabel}>{option.label}</span>
              <span style={styles.drawerOptionDesc}>{option.description}</span>
            </button>
          ))}
          <div style={styles.drawerCustomBox}>
            <textarea
              style={styles.drawerCustomInput}
              value={customAnswer}
              onChange={(event) => setCustomAnswer(event.target.value)}
              placeholder="自定义输入..."
              rows={3}
            />
            <button
              type="button"
              style={styles.drawerPrimaryBtn}
              disabled={!customAnswer.trim()}
              onClick={() => {
                onAnswerPlanQuestion(`自定义回答：${customAnswer.trim()}`);
                setCustomAnswer("");
              }}
            >
              发送自定义
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (planInteraction?.kind === "ready") {
    return (
      <div style={styles.drawer}>
        <div style={styles.drawerHeader}>
          <span style={styles.drawerTitle}>
            <FileText size={14} />
            计划已完成
          </span>
          <span style={styles.drawerPath}>{planInteraction.planPath}</span>
        </div>
        <div style={styles.drawerQuestion}>{planInteraction.title}</div>
        <div style={styles.drawerSummary}>{planInteraction.summary}</div>
        <div style={styles.drawerActionRow}>
          <button
            type="button"
            style={styles.drawerPrimaryBtn}
            disabled={implementingPlan}
            onClick={() => onImplementPlan(planInteraction)}
          >
            是，实施此计划
          </button>
          <button
            type="button"
            style={styles.drawerSecondaryBtn}
            disabled={implementingPlan}
            onClick={() => onImplementPlanWithClearedContext(planInteraction)}
          >
            清除上下文后实施
          </button>
          <button type="button" style={styles.drawerGhostBtn} onClick={onStayInPlanMode}>
            否，继续修改
          </button>
        </div>
      </div>
    );
  }

  if (checklist && checklist.items.length > 0) {
    return (
      <div style={styles.drawer}>
        <div style={styles.drawerHeader}>
          <span style={styles.drawerTitle}>
            <ClipboardList size={14} />
            本次任务规划步骤
          </span>
          <span style={styles.drawerPath}>
            {new Date(checklist.updatedAt).toLocaleTimeString()}
          </span>
        </div>
        {checklist.explanation && <div style={styles.drawerSummary}>{checklist.explanation}</div>}
        <div style={styles.checklistRows}>
          {checklist.items.map((item, index) => (
            <div key={item.id ?? `${item.step}-${index}`} style={styles.checklistRow}>
              <span
                style={styles.checklistStatus(item.status)}
                title={
                  item.status === "in_progress"
                    ? "正在执行"
                    : item.status === "completed"
                      ? "已完成"
                      : "等待执行"
                }
              >
                <span style={styles.checklistStatusDot(item.status)} />
              </span>
              <div style={styles.checklistContent}>
                <span style={styles.checklistText(item.status)}>{item.step}</span>
                {(item.agent || item.detail) && (
                  <span style={styles.checklistMeta}>
                    {item.agent ? getSubProcessAgentLabel(item.agent as AgentType) : "子任务"}
                    {item.detail ? ` · ${item.detail}` : ""}
                  </span>
                )}
              </div>
            </div>
          ))}
        </div>
      </div>
    );
  }

  return null;
});
