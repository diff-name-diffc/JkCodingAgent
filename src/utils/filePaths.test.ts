import { describe, expect, it } from "vitest";
import {
  buildSiblingPath,
  collapseMiddlePath,
  getPathBasename,
  getPathDirectory,
  getRelativePathDisplay,
  isSameOrChildPath,
} from "./filePaths";

describe("getPathBasename", () => {
  it("returns the segment after the last separator", () => {
    expect(getPathBasename("/a/b/c.ts")).toBe("c.ts");
  });

  it("returns the whole string when no separator exists", () => {
    expect(getPathBasename("README.md")).toBe("README.md");
  });
});

describe("getPathDirectory", () => {
  it("returns the leading directories with trailing separator", () => {
    expect(getPathDirectory("src/components/Foo.tsx")).toBe("src/components/");
  });

  it("returns an empty string for root-level files", () => {
    expect(getPathDirectory("README.md")).toBe("");
  });
});

describe("collapseMiddlePath", () => {
  it("keeps short paths untouched", () => {
    expect(collapseMiddlePath("src/a.ts", 40)).toBe("src/a.ts");
  });

  it("collapses the middle with a single ellipsis and respects the budget", () => {
    const path = "very/deeply/nested/directory/structure/with/Component.tsx";
    const collapsed = collapseMiddlePath(path, 24);
    expect(collapsed.length).toBeLessThanOrEqual(24);
    expect(collapsed).toContain("…");
    expect(collapsed.endsWith("Component.tsx".slice(-10))).toBe(true);
    expect(collapsed.startsWith("very")).toBe(true);
  });

  it("preserves the tail (filename side) more than the head", () => {
    const collapsed = collapseMiddlePath("aaaaaaaaaa/bbbbbbbbbb/cccccccccc.tsx", 15);
    const [head, tail] = collapsed.split("…");
    expect(tail.length).toBeGreaterThanOrEqual(head.length);
  });

  it("degrades gracefully for tiny budgets", () => {
    expect(collapseMiddlePath("abcdefgh", 2)).toBe("a…h");
  });
});

describe("buildSiblingPath", () => {
  it("replaces the basename under the same directory", () => {
    expect(buildSiblingPath("/a/b/old.ts", "new.ts")).toBe("/a/b/new.ts");
  });

  it("falls back to the bare name at the root", () => {
    expect(buildSiblingPath("old.ts", "new.ts")).toBe("new.ts");
  });
});

describe("isSameOrChildPath", () => {
  it("accepts the path itself and nested children", () => {
    expect(isSameOrChildPath("/a", "/a")).toBe(true);
    expect(isSameOrChildPath("/a", "/a/b/c.ts")).toBe(true);
    expect(isSameOrChildPath("/a", "/ab/c.ts")).toBe(false);
  });
});

describe("getRelativePathDisplay", () => {
  it("strips the root prefix for children", () => {
    expect(getRelativePathDisplay("/repo", "/repo/src/a.ts")).toBe("src/a.ts");
  });

  it("returns the input when outside the root", () => {
    expect(getRelativePathDisplay("/repo", "/other/a.ts")).toBe("/other/a.ts");
  });
});
