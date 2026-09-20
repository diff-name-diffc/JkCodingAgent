import { describe, expect, it } from "vitest";
import {
  autoPlaceInFrame,
  autoPlacePosition,
  finite,
  type AutoPlaceCursor,
} from "./arch-geometry";

function cursor(): AutoPlaceCursor {
  return { x: 0, y: 0, placed: false, frameCounts: new Map() };
}

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

describe("autoPlacePosition", () => {
  it("首个形状置于视口中心", () => {
    const c = cursor();
    const pos = autoPlacePosition({ x: 800, y: 400 }, c, 200, 100);
    expect(pos).toEqual({ x: 800 - 100, y: 400 - 50 });
    expect(c.placed).toBe(true);
  });

  it("其后形状沿 X 依次右移（固定步进 240）", () => {
    const c = cursor();
    autoPlacePosition({ x: 800, y: 400 }, c, 200, 100);
    const pos = autoPlacePosition({ x: 800, y: 400 }, c, 200, 100);
    expect(pos.x).toBe(800 - 100 + 240);
    expect(pos.y).toBe(400 - 50);
  });
});

describe("autoPlaceInFrame", () => {
  const frameBounds = { x: 100, y: 50 };

  it("槽位级联：同容器内 4 列换行", () => {
    const c = cursor();
    // 第 1 个：槽 0 → (100+24, 50+60)
    expect(autoPlaceInFrame(frameBounds, c, "shape:f1")).toEqual({ x: 124, y: 110 });
    // 第 2 个：槽 1 → x 加 208
    expect(autoPlaceInFrame(frameBounds, c, "shape:f1")).toEqual({ x: 124 + 208, y: 110 });
    // 第 5 个：槽 4 → 换行到第 2 行
    autoPlaceInFrame(frameBounds, c, "shape:f1");
    autoPlaceInFrame(frameBounds, c, "shape:f1");
    const fifth = autoPlaceInFrame(frameBounds, c, "shape:f1");
    expect(fifth).toEqual({ x: 124, y: 110 + 132 });
  });

  it("无 bounds 时回退到原点，且不同容器计数隔离", () => {
    const c = cursor();
    const pos = autoPlaceInFrame(undefined, c, "shape:unknown");
    expect(pos).toEqual({ x: 24, y: 60 });
    expect(c.frameCounts.get("shape:unknown")).toBe(1);
    expect(c.frameCounts.get("shape:f1")).toBeUndefined();
  });
});
