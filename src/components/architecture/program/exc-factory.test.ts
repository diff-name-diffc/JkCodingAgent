import { describe, expect, it } from "vitest";
import type { MutableElement } from "./exc-factory";
import {
  borderPointToward,
  cascadeDelete,
  cascadeMove,
  createArrowElement,
  createFrameElement,
  createShapeElement,
  createTextElement,
  estimateTextSize,
  recomputeArrowGeometry,
  registerArrowBindings,
  registerBoundText,
  type SceneDraft,
} from "./exc-factory";

const TEXT_STYLE = { fontSize: 20, fontFamily: 5, color: "#1e1e1e", align: "center" as const };

function rect(id: string, x = 0, y = 0, w = 200, h = 100): MutableElement {
  return createShapeElement(id, "rectangle", {
    x,
    y,
    width: w,
    height: h,
    strokeColor: "#1e1e1e",
    backgroundColor: "transparent",
    fillStyle: "solid",
    strokeWidth: 2,
    strokeStyle: "solid",
    roughness: 1,
    roundness: { type: 3 },
  });
}

describe("estimateTextSize", () => {
  it("CJK 按全宽、ASCII 按 0.55 估算", () => {
    const cjk = estimateTextSize("网关门", 20);
    expect(cjk.width).toBe(60);
    expect(cjk.height).toBe(25);
    const ascii = estimateTextSize("aa", 20);
    expect(ascii.width).toBeCloseTo(22);
  });

  it("多行取最宽行、行数决定高度", () => {
    const size = estimateTextSize("a\n网关门", 20);
    expect(size.width).toBe(60);
    expect(size.height).toBe(50);
  });
});

describe("borderPointToward", () => {
  it("返回朝目标方向的包围盒边界点", () => {
    const el = rect("shape:a", 0, 0, 200, 100); // 中心 (100, 50)
    expect(borderPointToward(el, { x: 400, y: 50 })).toEqual({ x: 200, y: 50 });
    expect(borderPointToward(el, { x: 100, y: 400 })).toEqual({ x: 100, y: 100 });
  });

  it("中心重合时退回中心", () => {
    const el = rect("shape:a", 0, 0, 200, 100);
    expect(borderPointToward(el, { x: 100, y: 50 })).toEqual({ x: 100, y: 50 });
  });
});

describe("箭头创建与重算", () => {
  function setup() {
    const draft: SceneDraft = new Map();
    const from = rect("shape:from", 0, 0, 200, 100);
    const to = rect("shape:to", 400, 0, 200, 100);
    draft.set(from.id, from);
    draft.set(to.id, to);
    const arrow = createArrowElement("shape:arr", {
      x: 0,
      y: 0,
      width: 0,
      height: 0,
      start: borderPointToward(from, { x: 500, y: 50 }),
      end: borderPointToward(to, { x: 100, y: 50 }),
      startBindingId: from.id,
      endBindingId: to.id,
      startArrowhead: null,
      endArrowhead: "arrow",
      strokeColor: "#1e1e1e",
      backgroundColor: "transparent",
      fillStyle: "solid",
      strokeWidth: 2,
      strokeStyle: "solid",
      roughness: 1,
      roundness: { type: 2 },
      labelPosition: 0.5,
    });
    draft.set(arrow.id, arrow);
    registerArrowBindings(draft, arrow);
    return { draft, from, to, arrow };
  }

  it("端点落在两端形状边框上，双向登记 boundElements", () => {
    const { from, to, arrow } = setup();
    expect(arrow.x).toBe(200); // from 右边框
    expect(arrow.y).toBe(50);
    expect(arrow.points[1][0]).toBe(200); // 到 to 左边框 (400) 的相对位移
    expect(from.boundElements).toEqual([{ id: "shape:arr", type: "arrow" }]);
    expect(to.boundElements).toEqual([{ id: "shape:arr", type: "arrow" }]);
  });

  it("形状移动后 recomputeArrowGeometry 重新贴合边框", () => {
    const { draft, to, arrow } = setup();
    to.x += 100; // to 移到 500
    recomputeArrowGeometry(arrow, draft);
    expect(arrow.points[1][0]).toBe(300);
  });
});

describe("绑定文本", () => {
  it("registerBoundText 双向登记并同步 frameId", () => {
    const draft: SceneDraft = new Map();
    const frame = createFrameElement(
      "shape:frame",
      {
        x: 0,
        y: 0,
        width: 400,
        height: 300,
        strokeColor: "#adb5bd",
        backgroundColor: "transparent",
        fillStyle: "solid",
        strokeWidth: 1,
        strokeStyle: "solid",
        roughness: 0,
        roundness: null,
      },
      "分组",
    );
    const box = rect("shape:box");
    box.frameId = frame.id;
    draft.set(frame.id, frame);
    draft.set(box.id, box);
    const label = createTextElement(0, 0, "服务", TEXT_STYLE, box.id);
    registerBoundText(box, label);
    expect(box.boundElements).toEqual([{ id: label.id, type: "text" }]);
    expect(label.containerId).toBe(box.id);
    expect(label.frameId).toBe(frame.id);
  });
});

describe("cascadeMove", () => {
  it("frame 带动子元素（含子元素的绑定文本），箭头不动", () => {
    const draft: SceneDraft = new Map();
    const frame = rect("shape:frame"); // 借用 rect 工厂，手动改类型模拟 frame
    frame.type = "frame" as never;
    const child = rect("shape:child", 10, 10);
    child.frameId = frame.id;
    const label = createTextElement(10, 10, "子", TEXT_STYLE, child.id);
    label.frameId = frame.id;
    const arrow = createArrowElement("shape:arr", {
      x: 0,
      y: 0,
      width: 0,
      height: 0,
      start: { x: 0, y: 0 },
      end: { x: 10, y: 0 },
      startBindingId: child.id,
      endBindingId: child.id,
      startArrowhead: null,
      endArrowhead: "arrow",
      strokeColor: "#1e1e1e",
      backgroundColor: "transparent",
      fillStyle: "solid",
      strokeWidth: 2,
      strokeStyle: "solid",
      roughness: 1,
      roundness: null,
      labelPosition: 0.5,
    });
    draft.set(frame.id, frame);
    draft.set(child.id, child);
    draft.set(label.id, label);
    draft.set(arrow.id, arrow);

    const moved = cascadeMove(draft, [frame.id], 100, 50);
    expect(moved.has(child.id)).toBe(true);
    expect(moved.has(label.id)).toBe(true);
    expect(child.x).toBe(110);
    expect(label.x).toBe(110);
    expect(arrow.x).toBe(0); // 箭头由 repair 重算，不在移动集合里
  });
});

describe("cascadeDelete", () => {
  it("删除形状带走绑定文本与绑定箭头，并清理幸存元素的引用", () => {
    const draft: SceneDraft = new Map();
    const a = rect("shape:a");
    const b = rect("shape:b", 400, 0);
    draft.set(a.id, a);
    draft.set(b.id, b);
    const label = createTextElement(0, 0, "A", TEXT_STYLE, a.id);
    registerBoundText(a, label);
    draft.set(label.id, label);
    const arrow = createArrowElement("shape:arr", {
      x: 0,
      y: 0,
      width: 0,
      height: 0,
      start: { x: 0, y: 0 },
      end: { x: 10, y: 0 },
      startBindingId: a.id,
      endBindingId: b.id,
      startArrowhead: null,
      endArrowhead: "arrow",
      strokeColor: "#1e1e1e",
      backgroundColor: "transparent",
      fillStyle: "solid",
      strokeWidth: 2,
      strokeStyle: "solid",
      roughness: 1,
      roundness: null,
      labelPosition: 0.5,
    });
    draft.set(arrow.id, arrow);
    registerArrowBindings(draft, arrow);

    const deleted = cascadeDelete(draft, [a.id]);
    expect(deleted.has(a.id)).toBe(true);
    expect(deleted.has(label.id)).toBe(true);
    expect(deleted.has(arrow.id)).toBe(true);
    expect(draft.has(arrow.id)).toBe(false);
    expect(b.boundElements).toEqual([]); // 幸存形状上的绑定引用被清理
  });

  it("删除 frame 带走子元素", () => {
    const draft: SceneDraft = new Map();
    const frame = rect("shape:frame");
    frame.type = "frame" as never;
    const child = rect("shape:child");
    child.frameId = frame.id;
    draft.set(frame.id, frame);
    draft.set(child.id, child);
    cascadeDelete(draft, [frame.id]);
    expect(draft.has(child.id)).toBe(false);
  });
});
