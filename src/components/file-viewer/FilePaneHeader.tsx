import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Check, Copy, Eye, PencilLine } from "lucide-react";
import { collapseMiddlePath, getPathDirectory, getRelativePathDisplay } from "../../utils/filePaths";

/** 文件 pane 的保存语义状态；idle 不渲染 pill。 */
export type FileSaveStatus = "idle" | "saving" | "saved" | "error" | "unsaved";

export function FileStatusPill({
  children,
  tone = "default",
}: {
  children: ReactNode;
  tone?: "default" | "success" | "error" | "warning";
}) {
  return <span className={`ai-file-status-pill is-${tone}`}>{children}</span>;
}

function savePillProps(status: FileSaveStatus): { label: string; tone: "default" | "success" | "error" | "warning" } | null {
  switch (status) {
    case "saving":
      return { label: "保存中", tone: "default" };
    case "saved":
      return { label: "已保存", tone: "success" };
    case "error":
      return { label: "保存失败", tone: "error" };
    case "unsaved":
      return { label: "未保存", tone: "warning" };
    default:
      return null;
  }
}

/**
 * 文件 pane 统一路径工具行（UI-16）：左侧为相对目录路径（长路径中间折叠，
 * 可复制完整路径），右侧为保存状态与 Markdown 预览切换。文件名由标签条
 * 承担，此处不再重复展示。
 */
export function FilePaneHeader({
  projectPath,
  filePath,
  saveStatus = "idle",
  meta = null,
  isMarkdown = false,
  previewMode = false,
  onTogglePreview,
}: {
  projectPath: string;
  filePath: string;
  saveStatus?: FileSaveStatus;
  /** 附加信息（图片 mime · 大小等）；空值不渲染。 */
  meta?: string | null;
  isMarkdown?: boolean;
  previewMode?: boolean;
  onTogglePreview?: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const copiedResetRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (copiedResetRef.current) {
        clearTimeout(copiedResetRef.current);
      }
    },
    [],
  );

  const handleCopyPath = useCallback(() => {
    navigator.clipboard.writeText(filePath).catch(() => undefined);
    setCopied(true);
    if (copiedResetRef.current) {
      clearTimeout(copiedResetRef.current);
    }
    copiedResetRef.current = setTimeout(() => setCopied(false), 1200);
  }, [filePath]);

  const relativeDir = getPathDirectory(getRelativePathDisplay(projectPath, filePath));
  const savePill = savePillProps(saveStatus);

  return (
    <div className="ai-file-pane-header">
      <div className="ai-file-path-line" title={filePath}>
        {relativeDir ? (
          <span className="ai-file-path-dir">{collapseMiddlePath(relativeDir, 48)}</span>
        ) : null}
        <button
          type="button"
          onClick={handleCopyPath}
          title="复制完整路径"
          aria-label="复制完整路径"
          className="ai-file-path-copy"
        >
          {copied ? <Check size={13} /> : <Copy size={13} />}
        </button>
      </div>

      <div className="ai-file-pane-actions">
        {meta ? <FileStatusPill>{meta}</FileStatusPill> : null}
        {savePill ? <FileStatusPill tone={savePill.tone}>{savePill.label}</FileStatusPill> : null}
        {isMarkdown && onTogglePreview ? (
          <button
            type="button"
            onClick={onTogglePreview}
            title={previewMode ? "切换到编辑" : "切换到预览"}
            className={previewMode ? "ai-file-preview-toggle is-active" : "ai-file-preview-toggle"}
          >
            {previewMode ? <PencilLine size={13} /> : <Eye size={13} />}
            {previewMode ? "编辑" : "预览"}
          </button>
        ) : null}
      </div>
    </div>
  );
}
