import { describe, expect, it } from "vitest";
import type { TLShapeId } from "tldraw";
import {
  finite,
  isPageId,
  styleProps,
  arrowStyleProps,
  autoPlaceInFrame,
  autoPlacePosition,
  type AutoPlaceCursor,
} from "./arch-geometry";

// TLShapeId 是 branded 类型，测试内以字符串直转。
const id = (s: string) => s as TLShapeId;

// tldraw Editor 的最小假实现：仅覆盖被测函数触碰的 API，其余以 `undefined` 兜底。
function fakeEditor(overrides: Record<string, unknown> = {}) {
  return {
    getViewportPageBounds: () => ({ midX: 800, midY: 400 }),
    getShapePageBounds: (shapeId: TLShapeId) =>
      shapeId === id("frame:1")
        ? { x: 100, y: 50, midX: 200, midY: 100, point: { x: 100, y: 50 } }
        : undefined,
    ...overrides,
  } as never;
}

function cursor(): AutoPlaceCursor {
  return { x: 0, y: 0, placed: false, frameCounts: new Map() };
}

describe("isPageId", () => {
  it("识别 page: 前缀", () => {
    expect(isPageId("page:abc")).toBe(true);
    expect(isPageId("shape:abc")).toBe(false);
    expect(isPageId("page")).toBe(false);
  });
});

describe("finite", () => {
  it("只接受有限数字", () => {
    expect(finite(3.5)).toBe(3.5);
    expect(finite(0)).toBe(0);
    expect(finite(undefined)).toBeUndefined();
    expect(finite(null)).toBeUndefined();
    expect(finite(NaN)).toBeUndefined();
    expect(finite(Infinity)).toBeUndefined();
    expect(finite("10")).toBeUndefined();
  });
});

describe("styleProps", () => {
  it("只携带已给出的样式字段", () => {
    expect(styleProps({})).toEqual({});
    expect(styleProps({ color: "red", size: "m" })).toEqual({ color: "red", size: "m" });
  });

  it("shape 样式含 fill/font，箭头样式不含", () => {
    const full = styleProps({ color: "red", fill: "solid", font: "mono", dash: "dashed" });
    expect(full).toEqual({ color: "red", fill: "solid", font: "mono", dash: "dashed" });
    const arrow = arrowStyleProps({ color: "red", fill: "solid", font: "mono", dash: "dashed" } as never);
    expect(arrow).toEqual({ color: "red", dash: "dashed" });
    expect(arrow).not.toHaveProperty("fill");
    expect(arrow).not.toHaveProperty("font");
  });
});

describe("autoPlacePosition", () => {
  it("首个形状置于视口中心", () => {
    const c = cursor();
    const pos = autoPlacePosition(fakeEditor(), c, 200, 100);
    expect(pos).toEqual({ x: 800 - 100, y: 400 - 50 });
    expect(c.placed).toBe(true);
  });

  it("其后形状沿 X 依次右移（固定步进 240）", () => {
    const c = cursor();
    autoPlacePosition(fakeEditor(), c, 200, 100);
    const pos = autoPlacePosition(fakeEditor(), c, 200, 100);
    expect(pos.x).toBe(800 - 100 + 240);
    expect(pos.y).toBe(400 - 50);
  });
});

describe("autoPlaceInFrame", () => {
  it("槽位级联：同容器内 4 列换行", () => {
    const editor = fakeEditor();
    const c = cursor();
    // 第 1 个：槽 0 → (100+24, 50+60)
    expect(autoPlaceInFrame(editor, c, id("frame:1"))).toEqual({ x: 124, y: 110 });
    // 第 2 个：槽 1 → x 加 208
    expect(autoPlaceInFrame(editor, c, id("frame:1"))).toEqual({ x: 124 + 208, y: 110 });
    // 第 5 个：槽 4 → 换行到第 2 行
    autoPlaceInFrame(editor, c, id("frame:1"));
    autoPlaceInFrame(editor, c, id("frame:1"));
    const fifth = autoPlaceInFrame(editor, c, id("frame:1"));
    expect(fifth).toEqual({ x: 124, y: 110 + 132 });
  });

  it("无 bounds 时回退到原点，且不同容器计数隔离", () => {
    const c = cursor();
    const pos = autoPlaceInFrame(fakeEditor(), c, id("frame:unknown"));
    expect(pos).toEqual({ x: 24, y: 60 });
    expect(c.frameCounts.get("frame:unknown")).toBe(1);
    expect(c.frameCounts.get("frame:1")).toBeUndefined();
  });
});
