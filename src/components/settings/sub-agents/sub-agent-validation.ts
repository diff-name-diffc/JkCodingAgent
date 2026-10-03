import type { SubAgentConfig } from "../../../types";

/**
 * sub-agent-validation.ts —— 子智能体编辑器的保存校验纯函数（自
 * SubAgentEditorDialog 拆出）。无组件 state 依赖，输入草稿、输出错误文案与
 * 需聚焦的编辑页签；校验边界常量与 JSX 展示共用单一出处。
 */

export type EditorTab = "basic" | "tools" | "runtime";

export const MAX_ITERATIONS = 200;
export const MIN_OUTPUT_TOKENS = 1024;
export const MAX_OUTPUT_TOKENS = 1048576;

export interface SubAgentDraftValidation {
  /** 校验失败时的错误文案；通过时缺省。 */
  error?: string;
  /** 需要聚焦的页签（工具集/运行时参数的越界错误时给出；基本信息错误不改页签）。 */
  focusTab?: EditorTab;
}

export function validateSubAgentDraft(draft: SubAgentConfig): SubAgentDraftValidation {
  if (!draft.agentId.trim()) {
    return { error: "Agent ID 不能为空" };
  }
  if (draft.agentId.length > 64) {
    return { error: "Agent ID 长度不能超过 64" };
  }
  if (!/^[a-z0-9][a-z0-9_-]*$/.test(draft.agentId)) {
    return {
      error: "Agent ID 仅支持小写字母、数字、下划线和短横线，且必须以小写字母或数字开头",
    };
  }
  if (!draft.agentName.trim()) {
    return { error: "显示名称不能为空" };
  }
  if (draft.agentName.length > 64) {
    return { error: "显示名称长度不能超过 64" };
  }
  if (!draft.description.trim()) {
    return { error: "功能描述不能为空" };
  }
  if (draft.description.length > 512) {
    return { error: "功能描述长度不能超过 512" };
  }
  if (!draft.systemPrompt.trim()) {
    return { error: "系统指令不能为空" };
  }
  if (draft.allowedTools.length === 0) {
    return { error: "至少选择一个工具", focusTab: "tools" };
  }
  if (draft.maxIterations < 1 || draft.maxIterations > MAX_ITERATIONS) {
    return { error: `最大迭代轮次必须在 1-${MAX_ITERATIONS} 之间`, focusTab: "runtime" };
  }
  if (draft.maxOutputTokens < MIN_OUTPUT_TOKENS || draft.maxOutputTokens > MAX_OUTPUT_TOKENS) {
    return {
      error: `最大输出 Token 必须在 ${MIN_OUTPUT_TOKENS}-${MAX_OUTPUT_TOKENS} 之间`,
      focusTab: "runtime",
    };
  }
  if (draft.temperature < 0 || draft.temperature > 2) {
    return { error: "Temperature 必须在 0-2 之间", focusTab: "runtime" };
  }
  if (draft.timeoutSecs < 1 || draft.timeoutSecs > 3600) {
    return { error: "超时时间必须在 1-3600 秒之间", focusTab: "runtime" };
  }
  return {};
}
