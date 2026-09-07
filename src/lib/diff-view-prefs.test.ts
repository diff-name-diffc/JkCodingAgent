import { describe, expect, it } from "vitest";
import {
  loadDiffViewMode,
  saveDiffViewMode,
  sanitizeDiffViewMode,
  type DiffViewStorage,
} from "./diff-view-prefs";

function mockStorage(initial: Record<string, string> = {}): DiffViewStorage & {
  dump: () => Record<string, string>;
} {
  const map = new Map<string, string>(Object.entries(initial));
  return {
    getItem: (key) => (map.has(key) ? (map.get(key) as string) : null),
    setItem: (key, value) => void map.set(key, value),
    dump: () => Object.fromEntries(map),
  };
}

const KEY = "jkcodingagent.git.diffViewMode.v1";

describe("sanitizeDiffViewMode", () => {
  it("接受合法值 split / unified", () => {
    expect(sanitizeDiffViewMode("split")).toBe("split");
    expect(sanitizeDiffViewMode("unified")).toBe("unified");
  });

  it("非法/未知/空值回退 unified", () => {
    expect(sanitizeDiffViewMode("side-by-side")).toBe("unified");
    expect(sanitizeDiffViewMode("")).toBe("unified");
    expect(sanitizeDiffViewMode(null)).toBe("unified");
    expect(sanitizeDiffViewMode(undefined)).toBe("unified");
    expect(sanitizeDiffViewMode(42)).toBe("unified");
  });
});

describe("loadDiffViewMode / saveDiffViewMode", () => {
  it("无存储值时默认 unified", () => {
    expect(loadDiffViewMode(mockStorage())).toBe("unified");
  });

  it("保存后读回同一模式（round-trip）", () => {
    const storage = mockStorage();
    saveDiffViewMode("split", storage);
    expect(loadDiffViewMode(storage)).toBe("split");
    saveDiffViewMode("unified", storage);
    expect(loadDiffViewMode(storage)).toBe("unified");
  });

  it("读到损坏值时经 sanitize 回退 unified", () => {
    const storage = mockStorage({ [KEY]: "garbage" });
    expect(loadDiffViewMode(storage)).toBe("unified");
  });

  it("storage 为 null（隐私模式）时 load 回退 unified、save 不抛错", () => {
    expect(loadDiffViewMode(null)).toBe("unified");
    expect(() => saveDiffViewMode("split", null)).not.toThrow();
  });

  it("storage.getItem 抛错时 load 兜底 unified", () => {
    const throwing: DiffViewStorage = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {},
    };
    expect(loadDiffViewMode(throwing)).toBe("unified");
  });

  it("storage.setItem 抛错时 save 静默降级", () => {
    const throwing: DiffViewStorage = {
      getItem: () => null,
      setItem: () => {
        throw new Error("quota");
      },
    };
    expect(() => saveDiffViewMode("split", throwing)).not.toThrow();
  });
});
