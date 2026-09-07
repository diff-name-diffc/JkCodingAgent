import { describe, expect, it } from "vitest";
import { isModelNotConfiguredError } from "./run-error-classify";

describe("isModelNotConfiguredError", () => {
  it("命中 validate_provider_completeness 的三条缺项错误串", () => {
    expect(
      isModelNotConfiguredError("错误：模型服务缺少 API Key，请先在设置中配置对应模型服务。"),
    ).toBe(true);
    expect(
      isModelNotConfiguredError(
        "错误：模型服务缺少 API 基础地址（Base URL），请先在设置中配置对应模型服务。",
      ),
    ).toBe(true);
    expect(
      isModelNotConfiguredError("错误：模型服务缺少模型名称，请先在设置中配置对应模型服务。"),
    ).toBe(true);
  });

  it("命中 plain_chat / project adapter 的 API Key 未配置串", () => {
    expect(
      isModelNotConfiguredError(
        "错误：聊天 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。",
      ),
    ).toBe(true);
    expect(
      isModelNotConfiguredError(
        "错误：项目编排 Agent 的 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。",
      ),
    ).toBe(true);
  });

  it("命中 architecture adapter 视觉模型未配置串", () => {
    expect(
      isModelNotConfiguredError(
        "错误：未配置视觉模型。请在设置中心「模型服务」添加视觉模型后重试。",
      ),
    ).toBe(true);
  });

  it("命中 build_architecture_agent 显式条目不完整分支", () => {
    expect(
      isModelNotConfiguredError(
        "错误：所选视觉模型的 URL 或模型名为空，且没有可用的视觉用途默认模型。请在设置中心「模型服务」补全后重试。",
      ),
    ).toBe(true);
  });

  it("对上层包装串（聊天/调度执行失败前缀）仍命中", () => {
    expect(
      isModelNotConfiguredError("聊天执行失败：错误：模型服务缺少 API Key，请先在设置中配置对应模型服务。"),
    ).toBe(true);
    expect(
      isModelNotConfiguredError("调度智能体执行失败：错误：聊天 LLM API Key 未配置。请在设置中配置。"),
    ).toBe(true);
  });

  it("普通网络 / HTTP / 鉴权错误不命中", () => {
    expect(isModelNotConfiguredError("错误：请求失败：HTTP 500 Internal Server Error")).toBe(false);
    expect(isModelNotConfiguredError("错误：网络连接超时，请检查网络后重试。")).toBe(false);
    expect(isModelNotConfiguredError("错误：API Key 无效或已过期（401 Unauthorized）")).toBe(false);
    expect(isModelNotConfiguredError("错误：模型返回内容解析失败")).toBe(false);
  });

  it("空串 / null / undefined 不命中", () => {
    expect(isModelNotConfiguredError("")).toBe(false);
    expect(isModelNotConfiguredError(null)).toBe(false);
    expect(isModelNotConfiguredError(undefined)).toBe(false);
  });
});
