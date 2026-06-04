import { useState, useEffect, useRef } from "react";
import { X } from "lucide-react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";

const CHAT_IMAGE_PROTOCOL = "chat-image://";

function isChatImageUri(src: string): boolean {
  return src.startsWith(CHAT_IMAGE_PROTOCOL);
}

function isLocalImagePath(src: string): boolean {
  return src.startsWith("/") || src.startsWith("file://");
}

interface MarkdownImageProps {
  src?: string;
  alt?: string;
}

export function MarkdownImage({ src, alt }: MarkdownImageProps) {
  const [isEnlarged, setIsEnlarged] = useState(false);
  const [resolvedSrc, setResolvedSrc] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const latestSrcRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    if (!src) {
      setResolvedSrc("");
      setLoading(false);
      setError(null);
      return;
    }

    latestSrcRef.current = src;
    let cancelled = false;

    if (isChatImageUri(src)) {
      setLoading(true);
      setError(null);
      invoke<{ imageId: string; path: string; mimeType: string }>("resolve_chat_image", {
        imageId: src.slice(CHAT_IMAGE_PROTOCOL.length),
      })
        .then((result) => {
          if (cancelled) return;
          if (latestSrcRef.current !== src) return;
          setResolvedSrc(convertFileSrc(result.path));
          setLoading(false);
        })
        .catch((err) => {
          if (cancelled) return;
          if (latestSrcRef.current !== src) return;
          console.error("resolve_chat_image failed:", err);
          setError("无法加载图片");
          setLoading(false);
        });
      return () => {
        cancelled = true;
      };
    }

    if (!isLocalImagePath(src)) {
      setResolvedSrc(src);
      setLoading(false);
      setError(null);
      return;
    }

    setLoading(true);
    setError(null);

    try {
      let path = src.startsWith("file://") ? src.slice(7) : src;
      // Markdown parsers may percent-encode non-ASCII chars in src;
      // decode first so convertFileSrc doesn't double-encode them.
      try {
        path = decodeURIComponent(path);
      } catch {
        // not encoded, use as-is
      }
      const assetUrl = convertFileSrc(path);
      setResolvedSrc(assetUrl);
      setLoading(false);
    } catch {
      setError("无法加载图片");
      setLoading(false);
    }
  }, [src]);

  if (loading) {
    return (
      <div className="markdown-image-thumbnail-wrap">
        <div className="markdown-image-loading">加载中...</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="markdown-image-thumbnail-wrap">
        <div className="markdown-image-error" title={error}>
          图片加载失败
        </div>
      </div>
    );
  }

  if (!resolvedSrc) return null;

  return (
    <>
      <div
        className="markdown-image-thumbnail-wrap"
        onClick={() => setIsEnlarged(true)}
        title="点击放大"
      >
        <img src={resolvedSrc} alt={alt} className="markdown-image-thumbnail" />
      </div>

      {isEnlarged && (
        <div className="markdown-image-overlay" onClick={() => setIsEnlarged(false)}>
          <button className="markdown-image-close-btn" onClick={() => setIsEnlarged(false)}>
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
}
