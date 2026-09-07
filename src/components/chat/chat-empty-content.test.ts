import { describe, expect, it } from "vitest";
import {
  PLAIN_CHAT_EMPTY_STATE,
  PROJECT_CHAT_EMPTY_STATE,
  resolveChatEmptyState,
} from "./chat-empty-content";

describe("resolveChatEmptyState", () => {
  it("plain 入口返回普通聊天文案", () => {
    expect(resolveChatEmptyState("plain")).toBe(PLAIN_CHAT_EMPTY_STATE);
  });

  it("project 入口返回项目文案", () => {
    expect(resolveChatEmptyState("project")).toBe(PROJECT_CHAT_EMPTY_STATE);
  });

  it("两个入口文案互不相同（不再共用通用空态）", () => {
    const plain = resolveChatEmptyState("plain");
    const project = resolveChatEmptyState("project");
    expect(plain.title).not.toBe(project.title);
    expect(plain.copy).not.toBe(project.copy);
    expect(plain.prompts).not.toEqual(project.prompts);
  });

  it("每个入口都有标题、副文案与 4 条起步提示", () => {
    for (const kind of ["plain", "project"] as const) {
      const content = resolveChatEmptyState(kind);
      expect(content.title?.length).toBeGreaterThan(0);
      expect(content.copy?.length).toBeGreaterThan(0);
      expect(content.prompts).toHaveLength(4);
      // 起步提示不得为空串或重复。
      const prompts = content.prompts ?? [];
      expect(prompts.every((prompt) => prompt.trim().length > 0)).toBe(true);
      expect(new Set(prompts).size).toBe(prompts.length);
    }
  });

  it("项目文案贴合项目语境（文件 / Git / 执行图），普通文案不绑定项目", () => {
    const projectPrompts = (PROJECT_CHAT_EMPTY_STATE.prompts ?? []).join(" ");
    expect(projectPrompts).toContain("项目");
    expect(projectPrompts).toContain("Git");
    expect(projectPrompts).toContain("执行图");

    const plainPrompts = (PLAIN_CHAT_EMPTY_STATE.prompts ?? []).join(" ");
    expect(plainPrompts).not.toContain("执行图");
    expect(PLAIN_CHAT_EMPTY_STATE.copy).toContain("不绑定项目");
  });
});
