import { useMemo } from "react";
import { LargeFileVirtualLine } from "./LargeFileVirtualLine";
import { LARGE_FILE_LINE_HEIGHT, type FileMeta } from "./large-file-types";
import { useLargeFileViewport } from "./useLargeFileViewport";

interface LargeFileViewerProps {
  active: boolean;
  sessionId: string;
  filePath: string;
  projectPath: string;
  meta: FileMeta;
}

/** 大文件（≥2MB）只读虚拟滚动查看器，行文本经 rope 会话分块拉取。 */
export function LargeFileViewer({
  active,
  sessionId,
  filePath,
  projectPath,
  meta,
}: LargeFileViewerProps) {
  const viewport = useLargeFileViewport({
    active,
    sessionId,
    filePath,
    projectPath,
    initialLineCount: meta.lineCount,
  });

  const gutterWidth = useMemo(
    () => Math.max(String(viewport.totalLines).length * 8 + 16, 48),
    [viewport.totalLines],
  );
  const sizeLabel = useMemo(
    () =>
      meta.sizeBytes >= 1024 * 1024
        ? `${(meta.sizeBytes / 1024 / 1024).toFixed(1)} MB`
        : `${(meta.sizeBytes / 1024).toFixed(1)} KB`,
    [meta.sizeBytes],
  );

  return (
    <div className="ai-large-file-viewer">
      <div className="ai-large-file-statusbar">
        <span>只读</span>
        <span>{sizeLabel}</span>
        <span>·</span>
        <span>{viewport.totalLines.toLocaleString()} 行</span>
      </div>

      <div
        ref={viewport.containerRef}
        onScroll={viewport.handleScroll}
        tabIndex={-1}
        className="ai-large-file-scroll chat-scroll"
        style={{ lineHeight: `${LARGE_FILE_LINE_HEIGHT}px` }}
      >
        <div
          className="ai-large-file-content-area"
          style={{ height: viewport.totalLines * LARGE_FILE_LINE_HEIGHT }}
        >
          {viewport.renderedLines.map(({ idx, text }) => (
            <LargeFileVirtualLine key={idx} idx={idx} text={text} gutterWidth={gutterWidth} />
          ))}
        </div>
      </div>
    </div>
  );
}
