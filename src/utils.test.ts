import { describe, expect, it } from "vitest";
import { fileName, fileDir } from "./utils";

describe("fileName", () => {
  it("取 POSIX 路径最后一段", () => {
    expect(fileName("src/components/Foo.tsx")).toBe("Foo.tsx");
    expect(fileName("/tmp/docs/a.pdf")).toBe("a.pdf");
  });

  it("识别 Windows 分隔符", () => {
    expect(fileName("C:\\docs\\b.docx")).toBe("b.docx");
  });

  it("无分隔符时返回原文", () => {
    expect(fileName("README.md")).toBe("README.md");
  });
});

describe("fileDir", () => {
  it("取目录前缀，根级文件返回空串", () => {
    expect(fileDir("src/components/Foo.tsx")).toBe("src/components");
    expect(fileDir("README.md")).toBe("");
  });
});
