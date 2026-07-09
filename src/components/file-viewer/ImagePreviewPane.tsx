import { useState } from "react";
import { AlertCircle } from "lucide-react";

function formatBytes(byteLength: number): string {
  if (byteLength < 1024) return `${byteLength} B`;
  if (byteLength < 1024 * 1024) return `${(byteLength / 1024).toFixed(1)} KB`;
  return `${(byteLength / 1024 / 1024).toFixed(1)} MB`;
}

export function ImagePreviewPane({
  src,
  fileName,
  mimeType,
  byteLength,
}: {
  src: string;
  fileName: string;
  mimeType: string;
  byteLength: number;
}) {
  const [loadError, setLoadError] = useState(false);

  if (loadError) {
    return (
      <div className="ai-image-preview-state">
        <AlertCircle size={24} strokeWidth={1.5} />
        <span>Image preview unavailable</span>
      </div>
    );
  }

  return (
    <div className="ai-image-preview-frame chat-scroll">
      <div className="ai-image-preview-stage">
        <img
          src={src}
          alt={fileName}
          draggable={false}
          onError={() => setLoadError(true)}
          className="ai-image-preview-img"
        />
        <div className="ai-image-preview-caption">
          {mimeType} · {formatBytes(byteLength)}
        </div>
      </div>
    </div>
  );
}
