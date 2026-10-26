/**
 * 回形钉选择器与剪贴板附件的分流：图片走既有 chat-images 管线（缩略图 +
 * 视觉输入），非图片文件仅在上传回调存在时交给父级（拷贝到会话临时目录）。
 */

export interface AttachmentFileTargets {
  onAttachImages?: (files: File[]) => void;
  onAttachFiles?: (files: File[]) => void;
}

export function isImageFile(file: File): boolean {
  return file.type.startsWith("image/");
}

export function dispatchAttachmentFiles(files: File[], targets: AttachmentFileTargets): void {
  const images = files.filter((file) => isImageFile(file));
  const documents = files.filter((file) => !isImageFile(file));
  if (images.length > 0) targets.onAttachImages?.(images);
  if (documents.length > 0) targets.onAttachFiles?.(documents);
}

/** 剪贴板里的文件项（粘贴截图 / 从访达复制的文件）；纯文本粘贴返回空。 */
export function clipboardFiles(data: DataTransfer | null): File[] {
  if (!data) return [];
  const files: File[] = [];
  for (const item of Array.from(data.items)) {
    if (item.kind === "file") {
      const file = item.getAsFile();
      if (file) files.push(file);
    }
  }
  return files;
}
