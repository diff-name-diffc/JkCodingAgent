import { describe, expect, it } from "vitest";
import type { DispatcherMessage, DispatcherMessageWire } from "../../types";
import { mergeDispatcherMessages } from "./dispatcherChatUtils";

describe("mergeDispatcherMessages", () => {
  it("把 Rust segmentsJson wire DTO 归一化为 UI segments", () => {
    // wire 载荷不含正文字段：content 由归一化从 segments 派生。
    const wire: DispatcherMessageWire = {
      id: "m1",
      workspaceId: "s1",
      role: "user",
      segmentsJson: JSON.stringify([{ id: "segment-1", type: "text", text: "hello" }]),
      createdAt: "2026-08-25T00:00:00Z",
    };

    expect(mergeDispatcherMessages([], [wire])).toEqual([
      expect.objectContaining({
        id: "m1",
        content: "hello",
        segments: [{ id: "segment-1", type: "text", text: "hello" }],
      }),
    ]);
  });
});

describe("mergeDispatcherMessages — 归一化身份缓存（UI-24b-4）", () => {
  const wire = (id: string, text: string, at = "2026-08-25T00:00:00Z"): DispatcherMessageWire => ({
    id,
    workspaceId: "s1",
    role: "user",
    segmentsJson: JSON.stringify([{ id: `seg-${id}`, type: "text", text }]),
    createdAt: at,
  });

  it("重入 merge：上一轮产物对象身份被复用（不重复 normalize）", () => {
    const first = mergeDispatcherMessages([], [wire("m1", "a"), wire("m2", "b", "2026-08-25T00:00:01Z")]);
    const second = mergeDispatcherMessages(first, [wire("m3", "c", "2026-08-25T00:00:02Z")]);
    // m1/m2 归一化产物应为同一对象引用（WeakMap 命中）
    expect(second.find((m) => m.id === "m1")).toBe(first.find((m) => m.id === "m1"));
    expect(second.find((m) => m.id === "m2")).toBe(first.find((m) => m.id === "m2"));
    expect(second).toHaveLength(3);
  });

  it("同 id incoming 覆盖 current：产物为新对象且内容取 incoming", () => {
    const first = mergeDispatcherMessages([], [wire("m1", "old")]);
    const second = mergeDispatcherMessages(first, [wire("m1", "new")]);
    expect(second).toHaveLength(1);
    expect(second[0].content).toBe("new");
    expect(second[0]).not.toBe(first[0]);
  });

  it("incoming 为空：返回 current 原数组（零开销短路保持）", () => {
    const first = mergeDispatcherMessages([], [wire("m1", "a")]);
    expect(mergeDispatcherMessages(first, [])).toBe(first);
  });

  it("排序语义不变：createdAt 优先、同刻按 id", () => {
    const merged = mergeDispatcherMessages(
      [],
      [
        wire("b", "2", "2026-08-25T00:00:01Z"),
        wire("a", "1", "2026-08-25T00:00:01Z"),
        wire("c", "0", "2026-08-25T00:00:00Z"),
      ],
    );
    expect(merged.map((m: DispatcherMessage) => m.id)).toEqual(["c", "a", "b"]);
  });
});
