import { describe, expect, it } from "vitest";
import type { ChatCategory, ChatSession } from "../../types";
import { resolveActiveChatCategory } from "./active-chat-category";

const category: ChatCategory = {
  id: "tech",
  name: "技术",
  icon: "Code2",
  color: "#297c70",
  sortOrder: 0,
  sessionCount: 1,
  createdAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:00Z",
};

const session: ChatSession = {
  id: "s1",
  title: "新对话",
  category: "tech",
  createdAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:00Z",
  keywords: [],
};

describe("resolveActiveChatCategory", () => {
  it("按会话分类 id 命中分类记录", () => {
    expect(resolveActiveChatCategory([session], [category], "s1")).toEqual(category);
  });

  it("无活跃会话或找不到会话时返回 null", () => {
    expect(resolveActiveChatCategory([session], [category], null)).toBeNull();
    expect(resolveActiveChatCategory([session], [category], "other")).toBeNull();
    expect(resolveActiveChatCategory([], [category], "s1")).toBeNull();
  });

  it("会话未挂分类（空串）时返回 null", () => {
    expect(
      resolveActiveChatCategory([{ ...session, category: "" }], [category], "s1"),
    ).toBeNull();
  });

  it("分类记录已删除（id 悬空）时返回 null，而不是造默认分类", () => {
    expect(resolveActiveChatCategory([session], [], "s1")).toBeNull();
  });
});
