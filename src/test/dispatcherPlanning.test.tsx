import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  buildPlanImplementationPrompt,
  buildPlanQuestionAnswer,
  InteractionDrawer,
} from "../components/DispatcherChat";
import type { ChecklistPlanState, PlanInteraction } from "../types";

describe("dispatcher planning UI", () => {
  it("renders three plan question options plus custom input", () => {
    const onAnswerPlanQuestion = vi.fn();
    render(
      <InteractionDrawer
        checklist={null}
        planInteraction={questionInteraction}
        implementingPlan={false}
        onAnswerPlanQuestion={onAnswerPlanQuestion}
        onImplementPlan={vi.fn()}
        onImplementPlanWithClearedContext={vi.fn()}
        onStayInPlanMode={vi.fn()}
      />,
    );

    expect(screen.getByText("方案 A")).toBeInTheDocument();
    expect(screen.getByText("方案 B")).toBeInTheDocument();
    expect(screen.getByText("方案 C")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("自定义输入...")).toBeInTheDocument();

    fireEvent.click(screen.getByText("方案 A"));
    expect(onAnswerPlanQuestion).toHaveBeenCalledWith("选择：方案 A\n说明：保守实现");
  });

  it("builds structured plan answer and implementation prompts", () => {
    expect(buildPlanQuestionAnswer(questionInteraction, "选择：方案 A")).toContain(
      "问题：怎么处理兼容性？",
    );

    const prompt = buildPlanImplementationPrompt("/repo/.jkcodingagent/plan/demo.md");
    expect(prompt).toContain("按照 Claude 和 Codex 各自擅长点派遣子任务");
    expect(prompt).toContain("不要重新规划步骤，也不要调用 update_plan");
    expect(prompt).not.toContain("# Demo");
    expect(prompt).toContain("mark_plan_implemented");
  });

  it("renders checklist execution states with explicit status icons", () => {
    render(
      <InteractionDrawer
        checklist={checklistState}
        planInteraction={null}
        implementingPlan={false}
        onAnswerPlanQuestion={vi.fn()}
        onImplementPlan={vi.fn()}
        onImplementPlanWithClearedContext={vi.fn()}
        onStayInPlanMode={vi.fn()}
      />,
    );

    expect(screen.getByText("本次任务规划步骤")).toBeInTheDocument();
    expect(screen.getByTitle("已完成")).toBeInTheDocument();
    expect(screen.getByTitle("正在执行")).toBeInTheDocument();
    expect(screen.getByTitle("等待执行")).toBeInTheDocument();
    expect(screen.getByText("Claude · 实现后端状态机")).toBeInTheDocument();
  });
});

const questionInteraction: Extract<PlanInteraction, { kind: "question" }> = {
  kind: "question",
  id: "q1",
  question: "怎么处理兼容性？",
  options: [
    { id: "a", label: "方案 A", description: "保守实现" },
    { id: "b", label: "方案 B", description: "完整迁移" },
    { id: "c", label: "方案 C", description: "仅做新路径" },
  ],
};

const checklistState: ChecklistPlanState = {
  updatedAt: "2026-05-09T12:00:00Z",
  items: [
    { id: "step_1", step: "审查代码", status: "completed" },
    {
      id: "step_2",
      step: "实现状态机",
      status: "in_progress",
      agent: "claude",
      detail: "实现后端状态机",
    },
    { id: "step_3", step: "验证链路", status: "pending" },
  ],
};
