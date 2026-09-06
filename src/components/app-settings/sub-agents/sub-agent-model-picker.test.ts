import { describe, expect, it } from "vitest";
import type { ModelLibraryEntry } from "../../../types";
import {
  PICKABLE_CATEGORIES,
  findMatchedLibraryEntry,
  pickableCategoryOptions,
  pickableEntries,
} from "./sub-agent-model-picker";

function entry(overrides: Partial<ModelLibraryEntry> = {}): ModelLibraryEntry {
  return {
    id: overrides.id ?? "id",
    category: overrides.category ?? "text",
    url: overrides.url ?? "https://api.example.com/v1",
    apiKey: overrides.apiKey ?? "sk-test",
    model: overrides.model ?? "gpt-test",
    enabled: overrides.enabled ?? true,
    ...(overrides.alias ? { alias: overrides.alias } : {}),
  };
}

describe("pickableCategoryOptions", () => {
  it("只暴露对话模型与视觉模型两个分类，且带中文 label", () => {
    expect(PICKABLE_CATEGORIES).toEqual(["text", "vision"]);
    expect(pickableCategoryOptions()).toEqual([
      { category: "text", label: "对话模型" },
      { category: "vision", label: "视觉模型" },
    ]);
  });
});

describe("pickableEntries", () => {
  it("按分类过滤并只返回启用条目", () => {
    const library = [
      entry({ id: "t1", category: "text", model: "b-model" }),
      entry({ id: "t2", category: "text", model: "a-model" }),
      entry({ id: "t3", category: "text", model: "off", enabled: false }),
      entry({ id: "v1", category: "vision", model: "vl" }),
      entry({ id: "i1", category: "image", model: "dall-e" }),
    ];
    const text = pickableEntries(library, "text");
    expect(text.map((e) => e.id)).toEqual(["t2", "t1"]); // 按 model 名排序，排除停用
    expect(pickableEntries(library, "vision").map((e) => e.id)).toEqual(["v1"]);
  });

  it("分类无启用条目时返回空数组", () => {
    expect(pickableEntries([entry({ category: "image" })], "text")).toEqual([]);
  });
});

describe("findMatchedLibraryEntry", () => {
  const library = [
    entry({ id: "t1", category: "text", url: "https://a/v1", model: "chat-a" }),
    entry({ id: "v1", category: "vision", url: "https://b/v1", model: "vision-b" }),
    entry({
      id: "t-off",
      category: "text",
      url: "https://c/v1",
      model: "chat-off",
      enabled: false,
    }),
    entry({ id: "i1", category: "image", url: "https://d/v1", model: "img-d" }),
  ];

  it("按 url+model 命中对话模型条目", () => {
    expect(
      findMatchedLibraryEntry(library, {
        apiBase: "https://a/v1",
        modelName: "chat-a",
      })?.id,
    ).toBe("t1");
  });

  it("跨分类命中视觉模型条目", () => {
    expect(
      findMatchedLibraryEntry(library, {
        apiBase: "https://b/v1",
        modelName: "vision-b",
      })?.id,
    ).toBe("v1");
  });

  it("匹配时忽略首尾空白", () => {
    expect(
      findMatchedLibraryEntry(library, {
        apiBase: "  https://a/v1  ",
        modelName: " chat-a ",
      })?.id,
    ).toBe("t1");
  });

  it("未命中返回 undefined", () => {
    expect(
      findMatchedLibraryEntry(library, {
        apiBase: "https://a/v1",
        modelName: "unknown",
      }),
    ).toBeUndefined();
  });

  it("停用条目不作为匹配结果", () => {
    expect(
      findMatchedLibraryEntry(library, {
        apiBase: "https://c/v1",
        modelName: "chat-off",
      }),
    ).toBeUndefined();
  });

  it("非可挑选分类（如 image）即使 url+model 相同也不匹配", () => {
    expect(
      findMatchedLibraryEntry(library, {
        apiBase: "https://d/v1",
        modelName: "img-d",
      }),
    ).toBeUndefined();
  });

  it("apiBase 或 modelName 为空时返回 undefined", () => {
    expect(findMatchedLibraryEntry(library, { apiBase: "", modelName: "chat-a" })).toBeUndefined();
    expect(findMatchedLibraryEntry(library, { apiBase: "https://a/v1" })).toBeUndefined();
    expect(findMatchedLibraryEntry(library, {})).toBeUndefined();
  });

  it("同 url+model 的多账号条目按 apiKey 消歧", () => {
    const multi = [
      entry({ id: "acc-1", url: "https://a/v1", model: "chat-a", apiKey: "sk-1" }),
      entry({ id: "acc-2", url: "https://a/v1", model: "chat-a", apiKey: "sk-2" }),
    ];
    expect(
      findMatchedLibraryEntry(multi, {
        apiBase: "https://a/v1",
        apiKey: "sk-2",
        modelName: "chat-a",
      })?.id,
    ).toBe("acc-2");
    // Key 与两个条目都不一致时不强行命中（手填或指向已删条目）。
    expect(
      findMatchedLibraryEntry(multi, {
        apiBase: "https://a/v1",
        apiKey: "sk-3",
        modelName: "chat-a",
      }),
    ).toBeUndefined();
  });
});
