import { describe, expect, it } from "vitest";
import {
  arrowheadValue,
  colorPair,
  dashProps,
  fillProps,
  fontFamilyValue,
  sizeProps,
  textAlignValue,
} from "./exc-style";

describe("colorPair", () => {
  it("缺省回退 black", () => {
    expect(colorPair(undefined)).toEqual(colorPair("black"));
  });

  it("每个 DSL 颜色词都有 stroke/bg 对", () => {
    expect(colorPair("blue")).toEqual({ stroke: "#1971c2", bg: "#a5d8ff" });
  });
});

describe("fillProps", () => {
  it("none → 透明", () => {
    expect(fillProps("none", "blue").backgroundColor).toBe("transparent");
  });

  it("semi → 同族浅色带 alpha", () => {
    expect(fillProps("semi", "blue").backgroundColor).toBe("#a5d8ff99");
    expect(fillProps("semi", "blue").fillStyle).toBe("solid");
  });

  it("pattern/lined-fill → hachure/cross-hatch", () => {
    expect(fillProps("pattern", "green").fillStyle).toBe("hachure");
    expect(fillProps("lined-fill", "green").fillStyle).toBe("cross-hatch");
  });
});

describe("dashProps", () => {
  it("draw → 手绘质感（roughness 1 + solid）", () => {
    expect(dashProps("draw")).toEqual({ strokeStyle: "solid", roughness: 1, transparentStroke: false });
  });

  it("solid/dashed/dotted → 利落几何线（roughness 0）", () => {
    expect(dashProps("dashed")).toEqual({ strokeStyle: "dashed", roughness: 0, transparentStroke: false });
    expect(dashProps("dotted").strokeStyle).toBe("dotted");
  });

  it("none → 透明描边", () => {
    expect(dashProps("none").transparentStroke).toBe(true);
  });
});

describe("sizeProps", () => {
  it("size 同时驱动描边宽度与字号", () => {
    expect(sizeProps("s")).toEqual({ strokeWidth: 1, fontSize: 16 });
    expect(sizeProps(undefined)).toEqual({ strokeWidth: 2, fontSize: 20 });
    expect(sizeProps("xl").fontSize).toBe(36);
  });
});

describe("fontFamilyValue / textAlignValue", () => {
  it("字体映射到 Excalidraw 字体族", () => {
    expect(fontFamilyValue("mono")).toBe(3); // Cascadia
    expect(fontFamilyValue("draw")).toBe(5); // Excalifont
  });

  it("对齐映射", () => {
    expect(textAlignValue("start")).toBe("left");
    expect(textAlignValue("middle")).toBe("center");
    expect(textAlignValue("end")).toBe("right");
    expect(textAlignValue(undefined)).toBe("center");
  });
});

describe("arrowheadValue", () => {
  it("常规映射", () => {
    expect(arrowheadValue("arrow")).toBe("arrow");
    expect(arrowheadValue("triangle")).toBe("triangle");
    expect(arrowheadValue("diamond")).toBe("diamond");
  });

  it("无对应取最近语义：square/pipe/bar → bar，inverted → triangle_outline", () => {
    expect(arrowheadValue("square")).toBe("bar");
    expect(arrowheadValue("pipe")).toBe("bar");
    expect(arrowheadValue("inverted")).toBe("triangle_outline");
  });

  it("none/缺省 → null", () => {
    expect(arrowheadValue("none")).toBeNull();
    expect(arrowheadValue(undefined)).toBeNull();
  });
});
