import { useState } from "react";
import { X } from "lucide-react";

export function MarkdownImage({ src, alt }: { src?: string; alt?: string }) {
  const [isEnlarged, setIsEnlarged] = useState(false);

  if (!src) return null;

  return (
    <>
      <div
        className="markdown-image-thumbnail-wrap"
        onClick={() => setIsEnlarged(true)}
        title="点击放大"
      >
        <img src={src} alt={alt} className="markdown-image-thumbnail" />
      </div>

      {isEnlarged && (
        <div className="markdown-image-overlay" onClick={() => setIsEnlarged(false)}>
          <button className="markdown-image-close-btn" onClick={() => setIsEnlarged(false)}>
            <X size={24} />
          </button>
          <div className="markdown-image-enlarged-container">
            <img
              src={src}
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
