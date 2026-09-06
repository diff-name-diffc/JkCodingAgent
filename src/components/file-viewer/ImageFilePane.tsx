import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertCircle } from "lucide-react";
import { ImagePreviewPane } from "./ImagePreviewPane";
import { FilePaneHeader } from "./FilePaneHeader";

type ImagePreviewData = {
  dataUrl: string;
  mimeType: string;
  byteLength: number;
};

/** 图片文件 pane：统一路径工具行 + 棋盘格预览（UI-16 并入共享头部）。 */
export function ImageFilePane({
  filePath,
  fileName,
  projectPath,
}: {
  filePath: string;
  fileName: string;
  projectPath: string;
}) {
  const [imagePreview, setImagePreview] = useState<ImagePreviewData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setImagePreview(null);
    setError(null);

    invoke<ImagePreviewData>("read_image_preview", { path: filePath, projectPath })
      .then((preview) => {
        if (!cancelled) {
          setImagePreview(preview);
          setLoading(false);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(String(err));
          setLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [filePath, projectPath]);

  return (
    <div className="ai-file-pane">
      <FilePaneHeader
        projectPath={projectPath}
        filePath={filePath}
        meta={
          imagePreview ? `${imagePreview.mimeType} · ${(imagePreview.byteLength / 1024).toFixed(1)} KB` : null
        }
      />

      <div className="ai-file-pane-body">
        {loading && <div className="ai-file-pane-state">加载中...</div>}
        {error && !loading && (
          <div className="ai-file-pane-state is-error">
            <AlertCircle size={28} strokeWidth={1.7} />
            <div>{error}</div>
          </div>
        )}
        {!loading && !error && imagePreview && (
          <ImagePreviewPane
            src={imagePreview.dataUrl}
            fileName={fileName}
            mimeType={imagePreview.mimeType}
            byteLength={imagePreview.byteLength}
          />
        )}
      </div>
    </div>
  );
}
