import { describe, expect, it } from "vitest";
import type { ArchitectureCanvasMeta } from "../../types/architecture";
import { formatCanvasTime, removeCanvasMeta, upsertCanvasMeta } from "./canvas-file-list";

function meta(id: string, updatedAt: number, name = id): ArchitectureCanvasMeta {
  return { id, name, createdAt: updatedAt, updatedAt };
}

describe("upsertCanvasMeta", () => {
  it("新条目按更新时间倒序插入", () => {
    const list = [meta("a", 100), meta("b", 300)];
    const next = upsertCanvasMeta(list, meta("c", 200));
    expect(next.map((m) => m.id)).toEqual(["b", "c", "a"]);
  });

  it("同 id 覆盖旧条目且不重复", () => {
    const list = [meta("a", 100), meta("b", 200)];
    const next = upsertCanvasMeta(list, meta("a", 400, " renamed "));
    expect(next).toHaveLength(2);
    expect(next[0]).toMatchObject({ id: "a", name: " renamed ", updatedAt: 400 });
  });

  it("不改动入参数组", () => {
    const list = [meta("a", 100)];
    upsertCanvasMeta(list, meta("b", 200));
    expect(list).toHaveLength(1);
  });
});

describe("removeCanvasMeta", () => {
  it("按 id 移除，缺失 id 原样返回", () => {
    const list = [meta("a", 100), meta("b", 200)];
    expect(removeCanvasMeta(list, "a").map((m) => m.id)).toEqual(["b"]);
    expect(removeCanvasMeta(list, "missing")).toHaveLength(2);
  });
});

describe("formatCanvasTime", () => {
  it("输出 yyyy-MM-dd HH:mm（补零）", () => {
    // 2026-10-16 08:05 本地时区
    const ts = new Date(2026, 9, 16, 8, 5).getTime();
    expect(formatCanvasTime(ts)).toBe("2026-10-16 08:05");
  });
});
