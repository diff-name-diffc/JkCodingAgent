import { describe, expect, it } from "vitest";
import type { SubAgentConfig } from "../../../types";
import {
  MAX_ITERATIONS,
  MAX_OUTPUT_TOKENS,
  MIN_OUTPUT_TOKENS,
  validateSubAgentDraft,
} from "./sub-agent-validation";

function draft(overrides: Partial<SubAgentConfig> = {}): SubAgentConfig {
  return {
    agentId: "my-agent",
    agentName: "My Agent",
    description: "测试用子智能体",
    systemPrompt: "你是助手",
    userPromptTemplate: "{{task}}",
    allowedTools: ["read_file"],
    modelConfig: { inheritFromParent: true },
    maxIterations: 60,
    maxOutputTokens: 4096,
    temperature: 0.7,
    timeoutSecs: 3600,
    enabled: true,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

describe("validateSubAgentDraft", () => {
  it("合法草稿通过，返回空对象", () => {
    expect(validateSubAgentDraft(draft())).toEqual({});
  });

  it("Agent ID 为空", () => {
    expect(validateSubAgentDraft(draft({ agentId: "  " }))).toEqual({
      error: "Agent ID 不能为空",
    });
  });

  it("Agent ID 超长", () => {
    expect(validateSubAgentDraft(draft({ agentId: "a".repeat(65) })).error).toContain("长度不能超过 64");
  });

  it("Agent ID 非法字符", () => {
    expect(validateSubAgentDraft(draft({ agentId: "Bad ID!" })).error).toContain("仅支持小写字母");
  });

  it("显示名称/功能描述/系统指令为空", () => {
    expect(validateSubAgentDraft(draft({ agentName: " " })).error).toBe("显示名称不能为空");
    expect(validateSubAgentDraft(draft({ description: " " })).error).toBe("功能描述不能为空");
    expect(validateSubAgentDraft(draft({ systemPrompt: " " })).error).toBe("系统指令不能为空");
  });

  it("功能描述超长 512", () => {
    expect(validateSubAgentDraft(draft({ description: "x".repeat(513) })).error).toContain("512");
  });

  it("未选择工具 → 聚焦工具集页签", () => {
    expect(validateSubAgentDraft(draft({ allowedTools: [] }))).toEqual({
      error: "至少选择一个工具",
      focusTab: "tools",
    });
  });

  it("运行时参数越界 → 聚焦运行时页签", () => {
    expect(validateSubAgentDraft(draft({ maxIterations: 0 }))).toEqual({
      error: `最大迭代轮次必须在 1-${MAX_ITERATIONS} 之间`,
      focusTab: "runtime",
    });
    expect(validateSubAgentDraft(draft({ maxIterations: MAX_ITERATIONS + 1 })).focusTab).toBe("runtime");
    expect(
      validateSubAgentDraft(draft({ maxOutputTokens: MIN_OUTPUT_TOKENS - 1 })).error,
    ).toContain("最大输出 Token");
    expect(
      validateSubAgentDraft(draft({ maxOutputTokens: MAX_OUTPUT_TOKENS + 1 })).focusTab,
    ).toBe("runtime");
    expect(validateSubAgentDraft(draft({ temperature: 2.1 })).error).toBe("Temperature 必须在 0-2 之间");
    expect(validateSubAgentDraft(draft({ temperature: -0.1 })).focusTab).toBe("runtime");
    expect(validateSubAgentDraft(draft({ timeoutSecs: 0 })).error).toContain("超时时间");
    expect(validateSubAgentDraft(draft({ timeoutSecs: 3601 })).focusTab).toBe("runtime");
  });

  it("边界值本身合法", () => {
    expect(
      validateSubAgentDraft(
        draft({
          maxIterations: MAX_ITERATIONS,
          maxOutputTokens: MAX_OUTPUT_TOKENS,
          temperature: 2,
          timeoutSecs: 3600,
        }),
      ),
    ).toEqual({});
  });
});
