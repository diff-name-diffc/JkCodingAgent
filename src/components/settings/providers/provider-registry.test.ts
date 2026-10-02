import { describe, expect, it } from "vitest";

import { bindPurpose, getPurposeBinding, modelCapabilityTags } from "./provider-registry";
import {
  CATEGORY_DEFS,
  ENTRY_CONTEXT_WINDOW_RANGE,
  ENTRY_MAX_TOKENS_RANGE,
  createEntry,
} from "./model-library";
import type { AhaSettingsV2 } from "../../../types";

describe("modelCapabilityTags：真实容量配置优先于模型名启发式", () => {
  it("未配置 contextWindow 时回退正则启发式", () => {
    expect(modelCapabilityTags("kimi-k2")).toContain("长上下文");
    expect(modelCapabilityTags("some-small-model")).not.toContain("长上下文");
  });

  it("配置了真实 contextWindow 时按数据判定（覆盖正则结论）", () => {
    // 名字命中长上下文正则、但真实窗口只有 32k → 不出徽标
    expect(modelCapabilityTags("qwen-long", 32_768)).not.toContain("长上下文");
    // 名字不命中正则、但真实窗口 1M → 出徽标
    expect(modelCapabilityTags("oa-qwen3.8-max", 1_000_000)).toContain("长上下文");
    // 阈值边界：200k 恰好命中
    expect(modelCapabilityTags("custom-model", 200_000)).toContain("长上下文");
    expect(modelCapabilityTags("custom-model", 199_999)).not.toContain("长上下文");
  });

  it("视觉徽标仍按模型名判定", () => {
    expect(modelCapabilityTags("qwen-vl-max")).toContain("视觉");
  });
});

describe("模型库容量字段的类目与缺省语义", () => {
  it("仅 text/vision 类目展示容量字段", () => {
    const capacityCategories = CATEGORY_DEFS.filter((def) => def.hasCapacityFields).map(
      (def) => def.category,
    );
    expect(capacityCategories).toEqual(["text", "vision"]);
  });

  it("createEntry 不携带容量字段（undefined = 后端缺省：省略 max_tokens / 窗口默认 1M）", () => {
    const entry = createEntry("text");
    expect(entry.maxTokens).toBeUndefined();
    expect(entry.contextWindow).toBeUndefined();
  });

  it("容量区间与后端 normalize_capacity 常量一致", () => {
    expect(ENTRY_MAX_TOKENS_RANGE).toEqual({ min: 1024, max: 1_048_576 });
    expect(ENTRY_CONTEXT_WINDOW_RANGE).toEqual({ min: 1024, max: 100_000_000 });
  });
});

describe("bindPurpose：聊天与项目是独立槽位", () => {
  // 回归背景：项目对话输入框的模型选择器曾固定写 chatChat——而后端项目
  // 运行按 AgentContext::Project 读 project 槽位，导致项目界面切换模型
  // 只改了普通聊天的绑定、对本会话不生效。此用例固化两个槽位互不串写。
  const emptySettings = (): AhaSettingsV2 => ({
    shared: {
      visionModelConfigs: [],
      imageModelConfigs: [],
      imageEditModelConfigs: [],
      asrModelConfigs: [],
      ttsModelConfigs: [],
      embeddingModelConfigs: [],
    },
    project: { chatModelConfigs: [], summaryModelConfigs: [], allowedTools: [] },
    chat: { chatModelConfigs: [], summaryModelConfigs: [], allowedTools: [] },
    contextDebug: false,
    review: {
      modelConfig: { url: "", apiKey: "", model: "", active: true },
      systemPrompt: "",
    },
    modelLibrary: [],
  });

  const entry = (id: string, model: string) => ({
    id,
    url: "https://api.example.com/v1",
    apiKey: "sk-test",
    model,
  });

  it("绑定 projectChat 不影响 chatChat 的读取，反之亦然", () => {
    const bound = bindPurpose(emptySettings(), "projectChat", entry("e1", "proj-model"));
    expect(getPurposeBinding(bound, "projectChat")?.model).toBe("proj-model");
    expect(getPurposeBinding(bound, "chatChat")).toBeNull();

    const swapped = bindPurpose(bound, "chatChat", entry("e2", "chat-model"));
    expect(getPurposeBinding(swapped, "chatChat")?.model).toBe("chat-model");
    // 聊天槽位变更不得覆盖项目槽位既有绑定。
    expect(getPurposeBinding(swapped, "projectChat")?.model).toBe("proj-model");
  });

  it("绑定携带 libraryId 引用（凭据与容量由库条目解析）", () => {
    const bound = bindPurpose(emptySettings(), "chatChat", entry("e1", "m"));
    expect(bound.chat.chatModelConfigs[0]?.libraryId).toBe("e1");
    expect(bound.chat.chatModelConfigs[0]?.active).toBe(true);
  });
});

describe("projectVerifier：验收是项目侧独立槽位", () => {
  // 验收模型此前复用项目摘要槽位，摘要网关故障时验收退化为「未能验收」且
  // 无法单独替换。此用例固化验收槽位与摘要槽位互不串写。
  const emptySettings = (): AhaSettingsV2 => ({
    shared: {
      visionModelConfigs: [],
      imageModelConfigs: [],
      imageEditModelConfigs: [],
      asrModelConfigs: [],
      ttsModelConfigs: [],
      embeddingModelConfigs: [],
    },
    project: { chatModelConfigs: [], summaryModelConfigs: [], allowedTools: [] },
    chat: { chatModelConfigs: [], summaryModelConfigs: [], allowedTools: [] },
    contextDebug: false,
    review: {
      modelConfig: { url: "", apiKey: "", model: "", active: true },
      systemPrompt: "",
    },
    modelLibrary: [],
  });

  const entry = (id: string, model: string) => ({
    id,
    url: "https://api.example.com/v1",
    apiKey: "sk-test",
    model,
  });

  it("绑定 projectVerifier 不影响 projectSummary，反之亦然", () => {
    const bound = bindPurpose(emptySettings(), "projectVerifier", entry("e1", "verifier-model"));
    expect(getPurposeBinding(bound, "projectVerifier")?.model).toBe("verifier-model");
    expect(getPurposeBinding(bound, "projectSummary")).toBeNull();
    expect(getPurposeBinding(bound, "chatChat")).toBeNull();

    const swapped = bindPurpose(bound, "projectSummary", entry("e2", "summary-model"));
    expect(getPurposeBinding(swapped, "projectSummary")?.model).toBe("summary-model");
    // 摘要槽位变更不得覆盖验收槽位既有绑定。
    expect(getPurposeBinding(swapped, "projectVerifier")?.model).toBe("verifier-model");
  });

  it("未配置验收槽位时读取为空数组（后端回退摘要槽位）", () => {
    const settings = emptySettings();
    expect(getPurposeBinding(settings, "projectVerifier")).toBeNull();
  });
});
