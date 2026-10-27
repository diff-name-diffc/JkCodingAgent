import { describe, expect, it } from "vitest";
import { __testing } from "./file-clipboard";

describe("normalizePaths", () => {
  it("去空与去重按原串比较，保留首尾空格", () => {
    expect(
      __testing.normalizePaths(["/a/b.txt", "/a/b.txt", "  ", "", "/a/c.txt", " /a/d.txt "]),
    ).toEqual(["/a/b.txt", "/a/c.txt", " /a/d.txt "]);
  });
});
