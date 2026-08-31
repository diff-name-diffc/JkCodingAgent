import { describe, expect, it } from "vitest";
import type { DispatcherMessageWire } from "../../types";
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
