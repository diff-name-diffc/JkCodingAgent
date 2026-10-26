import { describe, expect, it } from "vitest";
import {
  appendUploadedFileReferences,
  formatUploadSizeLimit,
  MAX_UPLOAD_FILE_BYTES,
  UPLOAD_REFERENCE_PREFIX,
} from "./upload-files";

describe("appendUploadedFileReferences", () => {
  it("空草稿直接以引用行开头", () => {
    expect(appendUploadedFileReferences("", ["/tmp/s/a.pdf"])).toBe(
      `${UPLOAD_REFERENCE_PREFIX}/tmp/s/a.pdf`,
    );
    expect(appendUploadedFileReferences("   \n", ["/tmp/s/a.pdf"])).toBe(
      `${UPLOAD_REFERENCE_PREFIX}/tmp/s/a.pdf`,
    );
  });

  it("非空草稿去掉尾部空白后另起一行追加，多文件各占一行", () => {
    expect(appendUploadedFileReferences("帮我看下这两个文件\n\n", ["/s/a.pdf", "/s/b.csv"])).toBe(
      `帮我看下这两个文件\n${UPLOAD_REFERENCE_PREFIX}/s/a.pdf\n${UPLOAD_REFERENCE_PREFIX}/s/b.csv`,
    );
  });

  it("无上传路径时原样返回草稿", () => {
    expect(appendUploadedFileReferences("draft  ", [])).toBe("draft  ");
  });
});

describe("formatUploadSizeLimit", () => {
  it("按 MB 展示上限", () => {
    expect(MAX_UPLOAD_FILE_BYTES).toBe(32 * 1024 * 1024);
    expect(formatUploadSizeLimit()).toBe("32 MB");
  });
});
