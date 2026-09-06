import { AlertCircle, ChevronDown, ChevronRight, RotateCcw } from "lucide-react";
import { fileDir, fileName, getGitStatusColor, getGitStatusLabel } from "../../utils";
import { FileGlyph } from "../../file-icons";

export interface GitFileChange {
  path: string;
  status: string;
  staged: boolean;
  /** 重命名条目的原路径（porcelain `R old -> new`），非重命名为 null。 */
  origin_path?: string | null;
}

/** 列表区就近错误行：真实错误信息 + 重试 + 可关闭（UI-17）。 */
export function ErrorRow({
  message,
  retry,
  onDismiss,
}: {
  message: string;
  retry: () => void;
  onDismiss: () => void;
}) {
  return (
    <div className="ai-git-inline-error">
      <AlertCircle size={13} className="ai-git-inline-error-icon" />
      <span className="ai-git-inline-error-message">{message}</span>
      <button type="button" onClick={retry} className="ai-git-inline-error-action" title="重试">
        <RotateCcw size={12} />
      </button>
      <button
        type="button"
        onClick={onDismiss}
        className="ai-git-inline-error-action"
        title="关闭"
      >
        ×
      </button>
    </div>
  );
}

export function TopSectionHeader({
  label,
  count,
  collapsed,
  onToggleCollapse,
}: {
  label: string;
  count: number;
  collapsed: boolean;
  onToggleCollapse: () => void;
}) {
  return (
    <div
      onClick={onToggleCollapse}
      className="ai-git-top-section"
    >
      <span className="ai-git-section-chevron">
        {collapsed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
      </span>
      <span className="ai-git-top-section-label">
        {label}
      </span>
      <span className="ai-git-count-pill">
        {count}
      </span>
    </div>
  );
}

export function SectionHeader({
  label,
  count,
  actionIcon,
  actionTitle,
  onAction,
}: {
  label: string;
  count: number;
  actionIcon?: string;
  actionTitle?: string;
  onAction?: () => void;
}) {
  return (
    <div className="ai-git-section-header">
      <span className="ai-git-section-label">{label}</span>
      <span className="ai-git-section-count">
        {count}
      </span>
      {onAction && (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onAction();
          }}
          title={actionTitle}
          className="ai-git-section-action"
        >
          {actionIcon}
        </button>
      )}
    </div>
  );
}

export function FileRow({
  change,
  isActive = false,
  onFileClick,
  onToggle,
}: {
  change: GitFileChange;
  /** 该行的 diff 标签当前为主区活动标签（UI-17 对应高亮）。 */
  isActive?: boolean;
  onFileClick: () => void;
  onToggle: (e: React.MouseEvent) => void;
}) {
  const name = fileName(change.path);
  const dir = fileDir(change.path);
  const color = getGitStatusColor(change.status);
  const label = getGitStatusLabel(change.status);
  const originName = change.origin_path ? fileName(change.origin_path) : null;
  const isRename = change.status.toUpperCase() === "R" && originName !== null && originName !== name;
  const displayName = isRename ? `${originName} → ${name}` : name;
  const titleText = isRename ? `${change.origin_path} → ${change.path}` : change.path;

  return (
    <div
      onClick={onFileClick}
      title={titleText}
      className={isActive ? "ai-git-file-row is-active" : "ai-git-file-row"}
    >
      {/* Status dot */}
      <span
        className="ai-git-status-dot"
        style={{ background: color }}
      />

      {/* Status letter */}
      <span
        className="ai-git-status-label"
        style={{
          color,
        }}
      >
        {label}
      </span>
      <FileGlyph path={change.path} size={20} />

      {/* Filename + dir */}
      <span className="ai-git-file-name-wrap">
        <span className="ai-git-file-name">
          {displayName}
        </span>
        {dir && (
          <span className="ai-git-file-dir">{dir}</span>
        )}
      </span>

      {/* Stage/unstage toggle on hover */}
      <button
        onClick={onToggle}
        title={change.staged ? "取消暂存" : "暂存"}
        className="ai-git-file-stage-button"
      >
        {change.staged ? "−" : "+"}
      </button>
    </div>
  );
}
