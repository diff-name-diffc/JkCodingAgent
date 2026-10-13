import { describe, expect, it } from "vitest";
import type { SubAgentRecord } from "../../../types";
import { fromBackendConfig, toBackendConfig } from "./sub-agent-config";

function record(config: Record<string, unknown>): SubAgentRecord {
  return {
    id: "browser-agent",
    name: "浏览器助手",
    description: "网页探索",
    configJson: JSON.stringify(config),
    enabled: true,
    globalEnabled: true,
    createdAt: 1,
    updatedAt: 2,
  };
}

describe("browser-use 配置映射", () => {
  it("旧配置缺省使用 DOM 文本，并保存固定空工具集合", () => {
    const config = fromBackendConfig(record({ allowed_tools: ["browser_click"] }))!;
    expect(config.browserUseVision).toBe(false);
    const saved = JSON.parse(toBackendConfig(config));
    expect(saved.browser_use_vision).toBe(false);
    expect(saved.allowed_tools).toEqual([]);
  });

  it.each(["browser_use_vision", "browserUseVision"])("读取 %s 并保留显式视觉选择", (key) => {
    const config = fromBackendConfig(record({ [key]: true }))!;
    expect(config.browserUseVision).toBe(true);
    expect(JSON.parse(toBackendConfig(config)).browser_use_vision).toBe(true);
  });

  it("snake_case 权威值 false 不被 camelCase true 覆盖", () => {
    const config = fromBackendConfig(
      record({ browser_use_vision: false, browserUseVision: true }),
    )!;
    expect(config.browserUseVision).toBe(false);
  });
});
