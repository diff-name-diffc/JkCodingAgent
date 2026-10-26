import { describe, expect, it, vi } from "vitest";
import { clipboardFiles, dispatchAttachmentFiles, isImageFile } from "./file-attachments";

function fakeFile(type: string, name = "f.bin"): File {
  return { type, name } as File;
}

describe("isImageFile", () => {
  it("按 MIME 前缀识别图片", () => {
    expect(isImageFile(fakeFile("image/png"))).toBe(true);
    expect(isImageFile(fakeFile("application/pdf"))).toBe(false);
    expect(isImageFile(fakeFile(""))).toBe(false);
  });
});

describe("dispatchAttachmentFiles", () => {
  it("图片与非图片分别路由到两条管线", () => {
    const onAttachImages = vi.fn();
    const onAttachFiles = vi.fn();
    dispatchAttachmentFiles(
      [
        fakeFile("image/png", "a.png"),
        fakeFile("application/pdf", "b.pdf"),
        fakeFile("image/jpeg", "c.jpg"),
      ],
      { onAttachImages, onAttachFiles },
    );
    expect(onAttachImages).toHaveBeenCalledWith([
      fakeFile("image/png", "a.png"),
      fakeFile("image/jpeg", "c.jpg"),
    ]);
    expect(onAttachFiles).toHaveBeenCalledWith([fakeFile("application/pdf", "b.pdf")]);
  });

  it("未声明文件上传回调时非图片文件被丢弃", () => {
    const onAttachImages = vi.fn();
    dispatchAttachmentFiles([fakeFile("image/png"), fakeFile("text/plain")], {
      onAttachImages,
    });
    expect(onAttachImages).toHaveBeenCalledTimes(1);
  });

  it("空列表不触发任何回调", () => {
    const onAttachImages = vi.fn();
    const onAttachFiles = vi.fn();
    dispatchAttachmentFiles([], { onAttachImages, onAttachFiles });
    expect(onAttachImages).not.toHaveBeenCalled();
    expect(onAttachFiles).not.toHaveBeenCalled();
  });
});

describe("clipboardFiles", () => {
  it("收集 kind=file 的剪贴板项", () => {
    const image = fakeFile("image/png");
    const pdf = fakeFile("application/pdf");
    const data = {
      items: [
        { kind: "string", getAsFile: () => null },
        { kind: "file", getAsFile: () => image },
        { kind: "file", getAsFile: () => pdf },
        { kind: "file", getAsFile: () => null },
      ],
    } as unknown as DataTransfer;
    expect(clipboardFiles(data)).toEqual([image, pdf]);
  });

  it("空数据返回空数组", () => {
    expect(clipboardFiles(null)).toEqual([]);
  });
});
