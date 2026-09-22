import { describe, expect, it } from "vitest";
import type { DispatcherMessage, DispatcherMessageWire } from "../../types";
import {
  buildOptimisticUserMessage,
  mergeDispatcherMessages,
  toErrorMessage,
} from "./dispatcherChatUtils";

describe("toErrorMessage", () => {
  it("reads Error.message", () => {
    expect(toErrorMessage(new Error("LLM 流式请求失败"))).toBe("LLM 流式请求失败");
  });

  it("keeps a plain string", () => {
    expect(toErrorMessage("connection refused")).toBe("connection refused");
  });

  it("reads message from a Tauri-style object payload", () => {
    expect(
      toErrorMessage({
        message: "LLM 请求失败，HTTP 401：Incorrect API key",
      }),
    ).toBe("LLM 请求失败，HTTP 401：Incorrect API key");
  });

  it("reads error field when message is absent", () => {
    expect(toErrorMessage({ error: "发送流式对话请求失败" })).toBe("发送流式对话请求失败");
  });

  it("stringifies other objects instead of [object Object]", () => {
    expect(toErrorMessage({ code: "TIMEOUT", status: 504 })).toBe(
      '{"code":"TIMEOUT","status":504}',
    );
  });
});

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

describe("mergeDispatcherMessages — 乐观 pending 消息替换", () => {
  const wire = (id: string, at: string): DispatcherMessageWire => ({
    id,
    workspaceId: "s1",
    role: "user",
    segmentsJson: JSON.stringify([{ id: `seg-${id}`, type: "text", text: "hi" }]),
    createdAt: at,
  });
  // current 侧必须是归一化产物：wire 先经一次 merge 进入。
  const history = (ids: Array<[string, string]>): DispatcherMessage[] =>
    mergeDispatcherMessages([], ids.map(([id, at]) => wire(id, at)));

  it("权威消息到达时丢弃 pending（不出现两条同轮用户消息）", () => {
    const optimistic = buildOptimisticUserMessage("s1", "hi", []);
    expect(optimistic.pending).toBe(true);

    const withPending = mergeDispatcherMessages(
      history([["m0", "2026-08-25T00:00:00Z"]]),
      [optimistic],
    );
    expect(withPending).toHaveLength(2);

    const afterAcknowledge = mergeDispatcherMessages(withPending, [
      wire("m1", "2026-08-25T00:00:01Z"),
    ]);
    expect(afterAcknowledge.map((m) => m.id)).toEqual(["m0", "m1"]);
  });

  it("pending 批次注入不误删既有 pending（注入场景 hasAuthoritative=false）", () => {
    const optimistic = buildOptimisticUserMessage("s1", "hi", []);
    const merged = mergeDispatcherMessages([optimistic], [optimistic]);
    expect(merged).toHaveLength(1);
    expect(merged[0].pending).toBe(true);
  });

  it("失败对账批次（不含该轮消息）同样清除 pending", () => {
    const optimistic = buildOptimisticUserMessage("s1", "hi", []);
    const withPending = mergeDispatcherMessages(
      history([["m0", "2026-08-25T00:00:00Z"]]),
      [optimistic],
    );
    const afterReconcile = mergeDispatcherMessages(withPending, [
      wire("m0", "2026-08-25T00:00:00Z"),
    ]);
    expect(afterReconcile.map((m) => m.id)).toEqual(["m0"]);
  });
});

describe("buildOptimisticUserMessage", () => {
  it("segment 顺序与发送管线一致：图片在前、文本在后", () => {
    const message = buildOptimisticUserMessage("s1", "hello", [
      {
        id: "img-seg",
        type: "image",
        imageId: "img-1",
        source: "user_paste",
        mimeType: "image/png",
      },
    ]);
    expect(message.role).toBe("user");
    expect(message.pending).toBe(true);
    expect(message.segments.map((segment) => segment.type)).toEqual(["image", "text"]);
    expect(message.content).toBe("hello");
  });

  it("空文本纯图片消息：content 为空串、只含图片段", () => {
    const message = buildOptimisticUserMessage("s1", "", [
      {
        id: "img-seg",
        type: "image",
        imageId: "img-1",
        source: "user_paste",
        mimeType: "image/png",
      },
    ]);
    expect(message.content).toBe("");
    expect(message.segments).toHaveLength(1);
  });
});
