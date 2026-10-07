import { memo, useState } from "react";
import { X } from "lucide-react";
import { convertFileSrc } from "@tauri-apps/api/core";

const CHAT_IMAGE_PROTOCOL = "chat-image://";

function isLocalImagePath(src: string): boolean {
  return src.startsWith("/") || src.startsWith("file://");
}

interface MarkdownImageProps {
  src?: string;
  alt?: string;
  /**
   * full：正文中的图片，撑满所在内容列宽（默认；AI 消息 / 文档渲染出口）；
   * thumb：用户消息附件等小图场景，小尺寸缩略、纵横比完整不裁剪。
   */
  variant?: "full" | "thumb";
}

/**
 * 聊天图片的唯一渲染出口。`chat-image://{image_id}` 经 Tauri 自定义
 * `chat-image` scheme（convertFileSrc 已按平台转换为
 * `chat-image://localhost/{id}` / `http://chat-image.localhost/{id}`）同步
 * 直出 <img src>——不再走 invoke resolve 两阶段渲染。本地路径分支仅为
 * 兼容旧消息 markdown 里的绝对路径引用（asset 协议）。
 *
 * 尺寸变体见 App.css `.markdown-image-thumbnail-wrap--full/--thumb`；
 * 输入框附件预览由 `.chat-attachment-preview` 作用域覆盖（prompt-input）。
 */
export const MarkdownImage = memo(function MarkdownImage({
  src,
  alt,
  variant = "full",
}: MarkdownImageProps) {
  const [isEnlarged, setIsEnlarged] = useState(false);
  const [failed, setFailed] = useState(false);
  const wrapClassName = `markdown-image-thumbnail-wrap markdown-image-thumbnail-wrap--${variant}`;

  let resolvedSrc = "";
  if (src?.startsWith(CHAT_IMAGE_PROTOCOL)) {
    resolvedSrc = convertFileSrc(src.slice(CHAT_IMAGE_PROTOCOL.length), "chat-image");
  } else if (src && isLocalImagePath(src)) {
    let path = src.startsWith("file://") ? src.slice("file://".length) : src;
    // Markdown parsers may percent-encode non-ASCII chars in src;
    // decode first so convertFileSrc doesn't double-encode them.
    try {
      path = decodeURIComponent(path);
    } catch {
      // not encoded, use as-is
    }
    resolvedSrc = convertFileSrc(path);
  } else if (src) {
    resolvedSrc = src;
  }

  if (!resolvedSrc) return null;

  if (failed) {
    return (
      <div className={wrapClassName}>
        <div className="markdown-image-error" title={src}>
          图片加载失败
        </div>
      </div>
    );
  }

  return (
    <>
      <div className={wrapClassName} onClick={() => setIsEnlarged(true)} title="点击放大">
        <img
          src={resolvedSrc}
          alt={alt}
          className="markdown-image-thumbnail"
          onError={() => setFailed(true)}
        />
      </div>

      {isEnlarged && (
        <div className="markdown-image-overlay" onClick={() => setIsEnlarged(false)}>
          <button
            type="button"
            className="markdown-image-close-btn"
            onClick={() => setIsEnlarged(false)}
            aria-label="关闭放大图片"
          >
            <X size={24} />
          </button>
          <div className="markdown-image-enlarged-container">
            <img
              src={resolvedSrc}
              alt={alt}
              className="markdown-image-enlarged"
              onClick={(e) => e.stopPropagation()}
            />
          </div>
        </div>
      )}
    </>
  );
});
