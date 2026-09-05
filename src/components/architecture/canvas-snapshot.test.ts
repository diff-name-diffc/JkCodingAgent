import { describe, expect, it } from "vitest";
import { formatCanvasSnapshot, MAX_SNAPSHOT_SHAPES, type SnapshotInput } from "./canvas-snapshot";

function makeInput(shapes: SnapshotInput["shapes"]): SnapshotInput {
  return {
    pageId: "page:main",
    viewport: { x: 0, y: 0, w: 1920, h: 1080 },
    shapes,
  };
}

describe("formatCanvasSnapshot", () => {
  it("returns empty string for empty canvas", () => {
    expect(formatCanvasSnapshot(makeInput([]))).toBe("");
  });

  it("renders header and shape lines with text and bounds", () => {
    const snapshot = formatCanvasSnapshot(
      makeInput([
        {
          id: "shape:abc",
          type: "geo",
          text: "API 网关",
          bounds: { x: 40, y: 60, w: 180, h: 64 },
        },
      ]),
    );
    const lines = snapshot.split("\n");
    expect(lines[0]).toContain("形状数: 1");
    expect(lines[0]).toContain("page:main");
    expect(lines[1]).toBe('[shape:abc] geo "API 网关" x=40 y=60 w=180 h=64');
  });

  it("marks shapes outside the viewport", () => {
    const snapshot = formatCanvasSnapshot(
      makeInput([
        { id: "shape:far", type: "note", bounds: { x: 5000, y: 5000, w: 200, h: 80 } },
      ]),
    );
    expect(snapshot).toContain("（视口外）");
  });

  it("sorts shapes in reading order (top-to-bottom rows, left-to-right)", () => {
    const snapshot = formatCanvasSnapshot(
      makeInput([
        { id: "shape:c", type: "geo", bounds: { x: 10, y: 305, w: 10, h: 10 } },
        { id: "shape:a", type: "geo", bounds: { x: 300, y: 10, w: 10, h: 10 } },
        { id: "shape:b", type: "geo", bounds: { x: 10, y: 15, w: 10, h: 10 } },
      ]),
    );
    const lines = snapshot.split("\n").slice(1);
    // shape:a 与 shape:b 同行（32px 桶内），b 更靠左；c 在下一行
    expect(lines[0]).toContain("shape:b");
    expect(lines[1]).toContain("shape:a");
    expect(lines[2]).toContain("shape:c");
  });

  it("truncates long shape text", () => {
    const longText = "甲".repeat(200);
    const snapshot = formatCanvasSnapshot(
      makeInput([
        { id: "shape:t", type: "text", text: longText, bounds: { x: 0, y: 0, w: 10, h: 10 } },
      ]),
    );
    expect(snapshot).toContain("…");
    expect(snapshot.length).toBeLessThan(longText.length);
  });

  it("aggregates shapes beyond the listing cap", () => {
    const shapes: SnapshotInput["shapes"] = [];
    for (let i = 0; i < MAX_SNAPSHOT_SHAPES + 30; i += 1) {
      shapes.push({ id: `shape:s${i}`, type: "geo", bounds: { x: i * 5, y: 0, w: 4, h: 4 } });
    }
    const snapshot = formatCanvasSnapshot(makeInput(shapes));
    expect(snapshot).toContain(`另有 30 个形状未列出`);
    expect(snapshot.split("\n").length).toBe(1 + MAX_SNAPSHOT_SHAPES + 1);
  });

  it("renders arrow connection ends instead of bounds", () => {
    const snapshot = formatCanvasSnapshot(
      makeInput([
        {
          id: "shape:arr1",
          type: "arrow",
          text: "HTTP",
          bounds: { x: 100, y: 100, w: 300, h: 10 },
          arrowEnds: { from: "shape:a", to: "shape:b" },
        },
      ]),
    );
    const line = snapshot.split("\n")[1];
    expect(line).toBe('[shape:arr1] arrow "HTTP" from=shape:a to=shape:b');
  });

  it("marks unbound arrow terminals as none and falls back to bounds when fully free", () => {
    const halfBound = formatCanvasSnapshot(
      makeInput([
        {
          id: "shape:arr2",
          type: "arrow",
          bounds: { x: 0, y: 0, w: 50, h: 50 },
          arrowEnds: { from: "shape:a" },
        },
      ]),
    );
    expect(halfBound).toContain("from=shape:a to=none");

    // arrowEnds 缺省（两端皆未连接的自由箭头）：退回位置尺寸表示。
    const free = formatCanvasSnapshot(
      makeInput([
        { id: "shape:arr3", type: "arrow", bounds: { x: 5, y: 6, w: 70, h: 8 } },
      ]),
    );
    expect(free).toContain("[shape:arr3] arrow x=5 y=6 w=70 h=8");
  });

  it("renders parent container and locked markers", () => {
    const snapshot = formatCanvasSnapshot(
      makeInput([
        {
          id: "shape:svc",
          type: "geo",
          text: "服务",
          bounds: { x: 10, y: 10, w: 100, h: 60 },
          parentId: "shape:frame1",
          locked: true,
        },
      ]),
    );
    const line = snapshot.split("\n")[1];
    expect(line).toContain("parent=shape:frame1");
    expect(line).toContain("locked");
  });

  it("includes user selection in the header", () => {
    const snapshot = formatCanvasSnapshot({
      ...makeInput([
        { id: "shape:a", type: "geo", bounds: { x: 0, y: 0, w: 10, h: 10 } },
      ]),
      selectedIds: ["shape:a"],
    });
    expect(snapshot.split("\n")[0]).toContain("选中: shape:a（用户当前选中）");

    const noneSelected = formatCanvasSnapshot(
      makeInput([{ id: "shape:a", type: "geo", bounds: { x: 0, y: 0, w: 10, h: 10 } }]),
    );
    expect(noneSelected).not.toContain("选中:");
  });
});
