import { describe, expect, it } from "vitest";
import { buildDwgCacheFingerprint } from "../lib/dwgCache";

describe("buildDwgCacheFingerprint", () => {
  it("文件指纹不变时生成同一缓存 key，变化时强制失效", () => {
    const base = {
      projectPath: "/repo",
      filePath: "/repo/sample.dwg",
      fileSize: 1024,
      fileMtime: 100,
      parserVersion: "dwg-worker-v1",
    };

    expect(buildDwgCacheFingerprint(base)).toBe(buildDwgCacheFingerprint({ ...base }));
    expect(buildDwgCacheFingerprint({ ...base, fileSize: 1025 })).not.toBe(
      buildDwgCacheFingerprint(base),
    );
    expect(buildDwgCacheFingerprint({ ...base, fileMtime: 101 })).not.toBe(
      buildDwgCacheFingerprint(base),
    );
    expect(buildDwgCacheFingerprint({ ...base, parserVersion: "dwg-worker-v2" })).not.toBe(
      buildDwgCacheFingerprint(base),
    );
  });
});
