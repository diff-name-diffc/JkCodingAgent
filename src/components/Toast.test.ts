import { describe, expect, it } from "vitest";
import { mergeToastList, toastDurationMs, type ToastItem } from "./Toast";

function item(id: number, kind: ToastItem["kind"], message: string): ToastItem {
  return { id, kind, message };
}

describe("mergeToastList", () => {
  it("追加新条目到末尾", () => {
    const prev = [item(1, "error", "a")];
    expect(mergeToastList(prev, item(2, "warning", "b"))).toEqual([
      item(1, "error", "a"),
      item(2, "warning", "b"),
    ]);
  });

  it("同 kind + 同文案去重：移除旧条目、新条目置尾（刷新时长）", () => {
    const prev = [item(1, "error", "保存失败"), item(2, "warning", "b")];
    expect(mergeToastList(prev, item(3, "error", "保存失败"))).toEqual([
      item(2, "warning", "b"),
      item(3, "error", "保存失败"),
    ]);
  });

  it("同文案不同 kind 不合并", () => {
    const prev = [item(1, "error", "同一句话")];
    expect(mergeToastList(prev, item(2, "success", "同一句话"))).toEqual([
      item(1, "error", "同一句话"),
      item(2, "success", "同一句话"),
    ]);
  });

  it("超出上限丢最旧", () => {
    const prev = [
      item(1, "error", "a"),
      item(2, "error", "b"),
      item(3, "error", "c"),
      item(4, "error", "d"),
    ];
    expect(mergeToastList(prev, item(5, "error", "e"))).toEqual([
      item(2, "error", "b"),
      item(3, "error", "c"),
      item(4, "error", "d"),
      item(5, "error", "e"),
    ]);
  });

  it("去重优先于截断：重复文案腾出空位时不丢其他条目", () => {
    const prev = [
      item(1, "error", "重复"),
      item(2, "warning", "b"),
      item(3, "error", "c"),
      item(4, "error", "d"),
    ];
    expect(mergeToastList(prev, item(5, "error", "重复"))).toEqual([
      item(2, "warning", "b"),
      item(3, "error", "c"),
      item(4, "error", "d"),
      item(5, "error", "重复"),
    ]);
  });
});

describe("toastDurationMs", () => {
  it("success 更快消失，error/warning 保持长时长", () => {
    expect(toastDurationMs("success")).toBe(2500);
    expect(toastDurationMs("error")).toBe(5000);
    expect(toastDurationMs("warning")).toBe(5000);
  });
});
