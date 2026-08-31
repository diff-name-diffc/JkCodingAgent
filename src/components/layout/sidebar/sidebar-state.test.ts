import { describe, expect, it } from "vitest";
import type { ChatCategory, ChatSession } from "../../../types";
import {
  categoryKey,
  groupSessionsByCategory,
  UNCATEGORIZED_CATEGORY,
  UNCATEGORIZED_LABEL,
} from "./sidebar-state";

function session(id: string, category: string): ChatSession {
  return {
    id,
    title: `session-${id}`,
    category,
    createdAt: "2026-08-30T00:00:00.000Z",
    updatedAt: "2026-08-30T00:00:00.000Z",
    keywords: [],
  };
}

function category(id: string, sortOrder: number, sessionCount = 0): ChatCategory {
  return {
    id,
    name: `分类-${id}`,
    color: "#000",
    icon: "Folder",
    sortOrder,
    sessionCount,
    createdAt: "2026-08-30T00:00:00.000Z",
    updatedAt: "2026-08-30T00:00:00.000Z",
  };
}

describe("sidebar 分类分组", () => {
  it("categoryKey 把空分类归入未分类键", () => {
    expect(categoryKey("")).toBe(UNCATEGORIZED_CATEGORY);
    expect(categoryKey(null)).toBe(UNCATEGORIZED_CATEGORY);
    expect(categoryKey("cat-1")).toBe("cat-1");
  });

  it("已知分类始终展示，未分类分组仅在有会话时展示", () => {
    const groups = groupSessionsByCategory([session("s1", "cat-1")], [
      category("cat-1", 0),
      category("cat-2", 1),
    ]);
    expect(groups.map((group) => group.id)).toEqual(["cat-1", "cat-2"]);

    const withUncategorized = groupSessionsByCategory(
      [session("s1", "cat-1"), session("s2", "")],
      [category("cat-1", 0)],
    );
    expect(withUncategorized.map((group) => group.id)).toEqual(["cat-1", UNCATEGORIZED_CATEGORY]);
    expect(withUncategorized[1]?.label).toBe(UNCATEGORIZED_LABEL);
  });

  it("指向不存在分类的会话落入未分类分组", () => {
    const groups = groupSessionsByCategory(
      [session("s1", "ghost-category")],
      [category("cat-1", 0)],
    );
    const uncategorized = groups.find((group) => group.id === UNCATEGORIZED_CATEGORY);
    expect(uncategorized?.sessions.map((item) => item.id)).toEqual(["s1"]);
  });

  it("按 sortOrder 排序分类，total 取实体计数与本地会话数的较大值", () => {
    const groups = groupSessionsByCategory(
      [session("s1", "cat-b"), session("s2", "cat-b"), session("s3", "cat-a")],
      [category("cat-b", 2, 5), category("cat-a", 1, 0)],
    );
    expect(groups.map((group) => group.id)).toEqual(["cat-a", "cat-b"]);
    expect(groups[1]?.total).toBe(5);
    expect(groups[0]?.total).toBe(1);
  });
});
