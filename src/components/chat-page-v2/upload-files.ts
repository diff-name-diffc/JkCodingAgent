/**
 * 普通聊天的文件上传（回形针选非图片文件）：拷贝到会话临时目录后，把绝对
 * 路径作为引用行追加进输入框，随消息文本发给模型；模型经文件工具按会话
 * 工作区路径读取。
 */

/** 与后端 `chat_uploads::MAX_UPLOAD_BYTES` 同口径（32 MB）。 */
export const MAX_UPLOAD_FILE_BYTES = 32 * 1024 * 1024;

export const UPLOAD_REFERENCE_PREFIX = "已上传文件：";

/** 引用行追加到现有草稿末尾；空草稿直接以引用行开头。 */
export function appendUploadedFileReferences(draft: string, paths: string[]): string {
  if (paths.length === 0) return draft;
  const lines = paths.map((path) => `${UPLOAD_REFERENCE_PREFIX}${path}`).join("\n");
  const trimmed = draft.replace(/\s+$/, "");
  return trimmed.length === 0 ? lines : `${trimmed}\n${lines}`;
}

export function formatUploadSizeLimit(): string {
  return `${MAX_UPLOAD_FILE_BYTES / (1024 * 1024)} MB`;
}
