import { describe, expect, it } from "vitest";

import { modelCapabilityTags } from "./provider-registry";
import {
  CATEGORY_DEFS,
  ENTRY_CONTEXT_WINDOW_RANGE,
  ENTRY_MAX_TOKENS_RANGE,
  createEntry,
} from "./model-library";

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
