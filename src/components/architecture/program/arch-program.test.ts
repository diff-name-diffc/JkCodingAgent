import { describe, expect, it } from "vitest";
import { validateArchProgram } from "./arch-program";

describe("validateArchProgram", () => {
  const validProgram = {
    version: 1,
    instructions: [
      { _type: "create_shape", ref: "gateway", shape: "geo", geo: "rectangle", text: "网关" },
      { _type: "create_shape", ref: "svc", shape: "note", text: "服务" },
      { _type: "create_arrow", from: "gateway", to: "svc", label: "HTTP" },
      { _type: "layout", mode: "grid", targets: ["gateway", "svc"], columns: 2 },
    ],
  };

  it("accepts a well-formed program", () => {
    const result = validateArchProgram(validProgram);
    expect(result.ok).toBe(true);
  });

  it("rejects wrong version and empty instructions", () => {
    expect(validateArchProgram({ version: 2, instructions: validProgram.instructions }).ok).toBe(
      false,
    );
    expect(validateArchProgram({ version: 1, instructions: [] }).ok).toBe(false);
  });

  it("rejects unknown instruction types", () => {
    const result = validateArchProgram({
      version: 1,
      instructions: [{ _type: "explode_shape", ref: "a" }],
    });
    expect(result.ok).toBe(false);
  });

  it("rejects duplicate refs", () => {
    const result = validateArchProgram({
      version: 1,
      instructions: [
        { _type: "create_shape", ref: "a", shape: "note" },
        { _type: "create_shape", ref: "a", shape: "note" },
      ],
    });
    expect(result.ok).toBe(false);
  });

  it("rejects geo shape without geo type", () => {
    const result = validateArchProgram({
      version: 1,
      instructions: [{ _type: "create_shape", ref: "a", shape: "geo" }],
    });
    expect(result.ok).toBe(false);
  });

  it("rejects arrow self-link", () => {
    const result = validateArchProgram({
      version: 1,
      instructions: [{ _type: "create_arrow", from: "a", to: "a" }],
    });
    expect(result.ok).toBe(false);
  });

  it("enforces move_shape mode exclusivity while allowing single-axis moves", () => {
    const base = { version: 1, instructions: [] as unknown[] };
    expect(
      validateArchProgram({
        ...base,
        instructions: [{ _type: "move_shape", target: "shape:x", x: 1, y: 1, dx: 1, dy: 1 }],
      }).ok,
    ).toBe(false);
    // 混用两族（即使各自只给一个轴）同样拒绝。
    expect(
      validateArchProgram({
        ...base,
        instructions: [{ _type: "move_shape", target: "shape:x", x: 1, dy: 1 }],
      }).ok,
    ).toBe(false);
    // 一个字段都不给 = 无意义指令，拒绝。
    expect(
      validateArchProgram({
        ...base,
        instructions: [{ _type: "move_shape", target: "shape:x" }],
      }).ok,
    ).toBe(false);
    // 单轴绝对/单轴相对均合法：未给出的轴保持不变。
    expect(
      validateArchProgram({
        ...base,
        instructions: [{ _type: "move_shape", target: "shape:x", x: 1 }],
      }).ok,
    ).toBe(true);
    expect(
      validateArchProgram({
        ...base,
        instructions: [{ _type: "move_shape", target: "shape:x", dx: 1, dy: 1 }],
      }).ok,
    ).toBe(true);
  });

  it("requires create_shape x/y both or neither", () => {
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "create_shape", ref: "a", shape: "note", x: 10 }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "create_shape", ref: "a", shape: "note", x: 10, y: 20 }],
      }).ok,
    ).toBe(true);
  });

  it("validates delete_shape targets", () => {
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "delete_shape", targets: ["shape:x", "shape:y"] }],
      }).ok,
    ).toBe(true);
    // 空列表与超过上限（>20）都拒绝；恰好 20（上限）通过。
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "delete_shape", targets: [] }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "delete_shape", targets: Array(20).fill("shape:x") }],
      }).ok,
    ).toBe(true);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "delete_shape", targets: Array(21).fill("shape:x") }],
      }).ok,
    ).toBe(false);
    // 非数组与非法目标引用（空串）同样拒绝。
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "delete_shape" } as unknown as never],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "delete_shape", targets: [""] }],
      }).ok,
    ).toBe(false);
  });

  it("rejects empty updates and frame nesting via into", () => {
    // 空 update 是静默无操作，与 Rust 权威层同口径拒绝。
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "update_shape", target: "shape:x" }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "update_arrow", target: "shape:x" }],
      }).ok,
    ).toBe(false);
    // frame 只能位于页面根：frame + into = 嵌套 frame，拒绝。
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "create_shape", ref: "a", shape: "frame", into: "outer" }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "create_shape", ref: "a", shape: "note", into: "outer" }],
      }).ok,
    ).toBe(true);
  });

  it("validates update_arrow targets and fields", () => {
    expect(
      validateArchProgram({
        version: 1,
        instructions: [
          { _type: "update_arrow", target: "shape:x", labelPosition: 0.3, color: "red" },
        ],
      }).ok,
    ).toBe(true);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "update_arrow", target: "", labelPosition: 0.3 }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "update_arrow", target: "shape:x", labelPosition: NaN }],
      }).ok,
    ).toBe(false);
  });

  it("rejects layout columns for non-grid modes", () => {
    const result = validateArchProgram({
      version: 1,
      instructions: [{ _type: "layout", mode: "row", targets: ["a", "b"], columns: 2 }],
    });
    expect(result.ok).toBe(false);
  });

  it("rejects invalid enum values instead of falling through silently", () => {
    // 形状类型与 geo 枚举：非法值不得穿透到应用层（未赋值的 partial 会
    // 直接传给 tldraw createShapes）。
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "create_shape", ref: "a", shape: "blob" }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "create_shape", ref: "a", shape: "geo", geo: "circle" }],
      }).ok,
    ).toBe(false);
    // 布局模式非法值若穿透，arch-layout 会静默落入 grid 分支。
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "layout", mode: "circle", targets: ["a", "b"] }],
      }).ok,
    ).toBe(false);
    // 样式/对齐/箭头装饰枚举。
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "create_shape", ref: "a", shape: "note", color: "pink" }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "update_shape", target: "shape:x", align: "justify" }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [
          { _type: "update_arrow", target: "shape:x", kind: "spline" },
        ],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [
          { _type: "update_arrow", target: "shape:x", arrowheadEnd: "harpoon" },
        ],
      }).ok,
    ).toBe(false);
  });

  it("validates layout numerics: gap, columns and origin", () => {
    const layout = (extra: Record<string, unknown>) => ({
      version: 1,
      instructions: [{ _type: "layout", mode: "grid", targets: ["a", "b"], ...extra }],
    });
    expect(validateArchProgram(layout({ gap: -1 })).ok).toBe(false);
    expect(validateArchProgram(layout({ gap: 501 })).ok).toBe(false);
    expect(validateArchProgram(layout({ columns: 0 })).ok).toBe(false);
    expect(validateArchProgram(layout({ columns: 9 })).ok).toBe(false);
    // columns 必须是整数（Rust 侧为整数类型）。
    expect(validateArchProgram(layout({ columns: 2.5 })).ok).toBe(false);
    expect(validateArchProgram(layout({ origin: { x: NaN, y: 0 } })).ok).toBe(false);
    expect(
      validateArchProgram(layout({ gap: 40, columns: 2, origin: { x: 0, y: 0 } })).ok,
    ).toBe(true);
  });

  it("validates labelPosition range and w/h lower bound", () => {
    // labelPosition 越界：Rust 权威层限 [0,1]，防御层同口径。
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "update_arrow", target: "shape:x", labelPosition: 1.5 }],
      }).ok,
    ).toBe(false);
    // w/h 下限与 Rust/schema 契约一致（[1, 2000]）。
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "create_shape", ref: "a", shape: "geo", geo: "rectangle", w: 0.5 }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [
          { _type: "create_shape", ref: "a", shape: "geo", geo: "rectangle", w: 2001 },
        ],
      }).ok,
    ).toBe(false);
  });

  it("validates create_shape.into as a target reference", () => {
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "create_shape", ref: "a", shape: "note", into: "frameBox" }],
      }).ok,
    ).toBe(true);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "create_shape", ref: "a", shape: "note", into: "" }],
      }).ok,
    ).toBe(false);
  });

  it("validates reparent targets and parent literal", () => {
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "reparent", targets: ["shape:x"], parent: "frameBox" }],
      }).ok,
    ).toBe(true);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "reparent", targets: ["shape:x"], parent: "page" }],
      }).ok,
    ).toBe(true);
    // parent 缺失或空串都拒绝（与 "page" 语义严格区分）。
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "reparent", targets: ["shape:x"] } as unknown as never],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "reparent", targets: ["shape:x"], parent: "" }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "reparent", targets: [], parent: "page" }],
      }).ok,
    ).toBe(false);
  });

  it("validates select_shapes and camera instructions", () => {
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "select_shapes", targets: ["shape:x", "shape:y"], zoom: true }],
      }).ok,
    ).toBe(true);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "select_shapes", targets: [] }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "camera", mode: "fit" }],
      }).ok,
    ).toBe(true);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "camera", mode: "point", point: { x: 10, y: 20 } }],
      }).ok,
    ).toBe(true);
    // fit 不携带 point；point 必须给出有限坐标。
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "camera", mode: "fit", point: { x: 1, y: 2 } }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "camera", mode: "point" }],
      }).ok,
    ).toBe(false);
    expect(
      validateArchProgram({
        version: 1,
        instructions: [{ _type: "camera", mode: "point", point: { x: NaN, y: 0 } }],
      }).ok,
    ).toBe(false);
  });
});
