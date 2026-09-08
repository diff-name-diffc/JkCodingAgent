import { describe, expect, it } from "vitest";
import {
  inferModelNotConfiguredCategory,
  isModelNotConfiguredError,
} from "./run-error-classify";

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

  it("命中工具级「模型未配置」串（第四批遗留：analyze_image / 图片生成 / 图片编辑）", () => {
    expect(
      isModelNotConfiguredError(
        "错误：视觉模型未配置，请先在设置中配置视觉模型后再使用 analyze_image",
      ),
    ).toBe(true);
    expect(isModelNotConfiguredError("错误：视觉模型缺少 API Key，请先在设置中配置")).toBe(true);
    expect(isModelNotConfiguredError("错误：图片生成 API Key 未配置，请先在设置中配置")).toBe(true);
    expect(isModelNotConfiguredError("错误：图片编辑 API Key 未配置，请先在设置中配置")).toBe(true);
    // browser 工具的视觉调用包装串（含既有 run 级 marker「LLM API Key 未配置」）
    expect(isModelNotConfiguredError("错误：LLM API Key 未配置，无法调用视觉模型")).toBe(true);
  });

  it("安全审查门禁串不命中（修复入口非模型服务分类页，刻意排除）", () => {
    expect(
      isModelNotConfiguredError("未配置安全审查，已拒绝执行命令。请先在应用设置中配置安全审查模型。"),
    ).toBe(false);
  });

  it("空串 / null / undefined 不命中", () => {
    expect(isModelNotConfiguredError("")).toBe(false);
    expect(isModelNotConfiguredError(null)).toBe(false);
    expect(isModelNotConfiguredError(undefined)).toBe(false);
  });
});

describe("inferModelNotConfiguredCategory（工具级深链分类推断）", () => {
  it("视觉类工具串推断 vision", () => {
    expect(
      inferModelNotConfiguredCategory(
        "错误：视觉模型未配置，请先在设置中配置视觉模型后再使用 analyze_image",
      ),
    ).toBe("vision");
    expect(inferModelNotConfiguredCategory("错误：视觉模型缺少 API Key，请先在设置中配置")).toBe(
      "vision",
    );
    expect(inferModelNotConfiguredCategory("错误：LLM API Key 未配置，无法调用视觉模型")).toBe(
      "vision",
    );
  });

  it("图片生成 / 图片编辑串分别推断 image / imageEdit", () => {
    expect(inferModelNotConfiguredCategory("错误：图片生成 API Key 未配置，请先在设置中配置")).toBe(
      "image",
    );
    expect(inferModelNotConfiguredCategory("错误：图片编辑 API Key 未配置，请先在设置中配置")).toBe(
      "imageEdit",
    );
  });

  it("run 级串不推断分类（发起点自知分类，返回 null 由调用方兜底）", () => {
    expect(
      inferModelNotConfiguredCategory("错误：模型服务缺少 API Key，请先在设置中配置对应模型服务。"),
    ).toBeNull();
    expect(
      inferModelNotConfiguredCategory("错误：聊天 LLM API Key 未配置。请在设置中配置。"),
    ).toBeNull();
    expect(inferModelNotConfiguredCategory("错误：未配置视觉模型。请在设置中心添加后重试。")).toBeNull();
  });

  it("未命中 / 空值返回 null", () => {
    expect(inferModelNotConfiguredCategory("错误：网络连接超时")).toBeNull();
    expect(inferModelNotConfiguredCategory("")).toBeNull();
    expect(inferModelNotConfiguredCategory(null)).toBeNull();
    expect(inferModelNotConfiguredCategory(undefined)).toBeNull();
  });
});
