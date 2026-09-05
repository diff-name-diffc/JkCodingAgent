import { describe, expect, it } from "vitest";
import { layoutShapes, DEFAULT_LAYOUT_GAP } from "./arch-layout";

const origin = { x: 100, y: 200 };

describe("layoutShapes row", () => {
  it("accumulates horizontally with gap and centers cross axis by default", () => {
    const positions = layoutShapes(
      [
        { id: "a", w: 100, h: 40 },
        { id: "b", w: 200, h: 80 },
      ],
      { mode: "row", origin },
    );
    expect(positions.get("a")).toEqual({ x: 100, y: 200 + 20 });
    expect(positions.get("b")).toEqual({ x: 100 + 100 + DEFAULT_LAYOUT_GAP, y: 200 });
  });

  it("aligns start and end on cross axis", () => {
    const items = [
      { id: "a", w: 100, h: 40 },
      { id: "b", w: 100, h: 80 },
    ];
    const start = layoutShapes(items, { mode: "row", origin, align: "start" });
    expect(start.get("a")?.y).toBe(200);
    const end = layoutShapes(items, { mode: "row", origin, align: "end" });
    expect(end.get("a")?.y).toBe(240);
  });
});

describe("layoutShapes column", () => {
  it("accumulates vertically with gap", () => {
    const positions = layoutShapes(
      [
        { id: "a", w: 100, h: 40 },
        { id: "b", w: 200, h: 60 },
      ],
      { mode: "column", origin, gap: 20 },
    );
    expect(positions.get("a")).toEqual({ x: 150, y: 200 });
    expect(positions.get("b")).toEqual({ x: 100, y: 200 + 40 + 20 });
  });
});

describe("layoutShapes grid", () => {
  const four = [
    { id: "a", w: 100, h: 40 },
    { id: "b", w: 160, h: 40 },
    { id: "c", w: 120, h: 80 },
    { id: "d", w: 140, h: 60 },
  ];

  it("uses sqrt column count by default and centers cells", () => {
    const positions = layoutShapes(four, { mode: "grid", origin });
    // 2 列 2 行；列宽取列内最大宽 [120(a,c), 160(b,d)]，行高 [40, 80]
    expect(positions.get("a")).toEqual({ x: 100, y: 200 });
    expect(positions.get("b")).toEqual({ x: 100 + 120 + DEFAULT_LAYOUT_GAP, y: 200 });
    expect(positions.get("c")).toEqual({ x: 100, y: 200 + 40 + DEFAULT_LAYOUT_GAP });
    // d 高 60，在行高 80 中垂直居中 → 偏移 10
    expect(positions.get("d")).toEqual({
      x: 100 + 120 + DEFAULT_LAYOUT_GAP,
      y: 200 + 40 + DEFAULT_LAYOUT_GAP + 10,
    });
  });

  it("honors explicit columns and wraps rows", () => {
    const positions = layoutShapes(four, { mode: "grid", origin, columns: 3 });
    // 第 4 个形状换到第 2 行第 1 列
    expect(positions.get("d")?.y).toBeGreaterThan(positions.get("a")!.y);
    expect(positions.get("d")?.x).toBe(100);
  });

  it("clamps columns to item count", () => {
    const positions = layoutShapes(
      [
        { id: "a", w: 50, h: 50 },
        { id: "b", w: 50, h: 50 },
      ],
      { mode: "grid", origin, columns: 8 },
    );
    expect(positions.get("b")?.y).toBe(200);
  });
});

describe("layoutShapes edge cases", () => {
  it("returns empty map for empty input", () => {
    expect(layoutShapes([], { mode: "row", origin }).size).toBe(0);
  });
});
