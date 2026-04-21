import { describe, expect, it } from "vitest";
import { buildNormalizedDwgIndex } from "../lib/dwgNormalize";

describe("buildNormalizedDwgIndex", () => {
  it("归一化常见实体并汇总 bounds、文本样本和块引用", () => {
    const database = {
      entities: [
        {
          type: "LINE",
          handle: "10",
          layer: "A-WALL",
          startPoint: { x: 0, y: 0 },
          endPoint: { x: 20, y: 10 },
        },
        {
          type: "TEXT",
          handle: "20",
          layer: "A-TEXT",
          startPoint: { x: 5, y: 8 },
          text: "轴线 1",
        },
        {
          type: "INSERT",
          handle: "30",
          layer: "A-BLOCK",
          insertionPoint: { x: 12, y: 6 },
          name: "ROOM_TAG",
        },
      ],
    } as unknown;

    const normalized = buildNormalizedDwgIndex(
      database as never,
      "/repo/sample.dwg",
      "dwg-worker-v1",
      2,
    );

    expect(normalized.summary.filePath).toBe("/repo/sample.dwg");
    expect(normalized.summary.parserVersion).toBe("dwg-worker-v1");
    expect(normalized.summary.totalEntities).toBe(3);
    expect(normalized.summary.unknownEntityCount).toBe(2);
    expect(normalized.summary.bounds).toEqual({
      minX: 0,
      minY: 0,
      maxX: 20,
      maxY: 10,
    });
    expect(normalized.summary.layers).toEqual([
      { name: "A-BLOCK", entityCount: 1 },
      { name: "A-TEXT", entityCount: 1 },
      { name: "A-WALL", entityCount: 1 },
    ]);
    expect(normalized.summary.textSamples).toEqual(["轴线 1"]);
    expect(normalized.summary.blocks).toEqual([{ name: "ROOM_TAG", count: 1 }]);
    expect(normalized.entities[0].bbox).toEqual({
      minX: 0,
      minY: 0,
      maxX: 20,
      maxY: 10,
    });
  });
});
