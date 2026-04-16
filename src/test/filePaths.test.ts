import { describe, expect, it } from "vitest";
import {
  buildSiblingPath,
  getPathBasename,
  getRelativePathDisplay,
  isSameOrChildPath,
  replacePathPrefix,
} from "../utils/filePaths";

describe("file path helpers", () => {
  it("支持 unix 路径的同级重命名与相对路径展示", () => {
    expect(buildSiblingPath("/repo/src/app.tsx", "main.tsx")).toBe("/repo/src/main.tsx");
    expect(getPathBasename("/repo/src/app.tsx")).toBe("app.tsx");
    expect(getRelativePathDisplay("/repo", "/repo/src/app.tsx")).toBe("src/app.tsx");
  });

  it("支持 windows 路径的子级判断与前缀替换", () => {
    expect(isSameOrChildPath("C:\\repo\\src", "C:\\repo\\src\\pages\\home.tsx")).toBe(true);
    expect(replacePathPrefix("C:\\repo\\src\\pages\\home.tsx", "C:\\repo\\src", "C:\\repo\\app")).toBe(
      "C:\\repo\\app\\pages\\home.tsx",
    );
  });

  it("对不匹配的路径保持原值", () => {
    expect(isSameOrChildPath("/repo/src", "/repo/server/index.ts")).toBe(false);
    expect(replacePathPrefix("/repo/server/index.ts", "/repo/src", "/repo/app")).toBe(
      "/repo/server/index.ts",
    );
  });
});
