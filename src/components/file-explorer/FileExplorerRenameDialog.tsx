import { useEffect, useMemo, useRef, useState } from "react";
import { X } from "lucide-react";
import { buildSiblingPath, getRelativePathDisplay } from "../../utils/filePaths";

type RenameTarget = {
  path: string;
  name: string;
  isDir: boolean;
};

function validateNextName(nextName: string) {
  if (!nextName.trim()) {
    return "名称不能为空";
  }

  if (nextName === "." || nextName === "..") {
    return "名称不能是 . 或 ..";
  }

  if (/[/\\]/.test(nextName)) {
    return "名称不能包含路径分隔符";
  }

  return null;
}

export function FileExplorerRenameDialog({
  projectPath,
  target,
  saving,
  onClose,
  onSubmit,
}: {
  projectPath: string;
  target: RenameTarget | null;
  saving: boolean;
  onClose: () => void;
  onSubmit: (nextName: string) => void | Promise<void>;
}) {
  const [nextName, setNextName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!target) {
      setNextName("");
      setError(null);
      return;
    }

    setNextName(target.name);
    setError(null);
  }, [target]);

  useEffect(() => {
    if (!target) {
      return;
    }

    const frameId = window.requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });

    return () => {
      window.cancelAnimationFrame(frameId);
    };
  }, [target]);

  const nextPathPreview = useMemo(() => {
    if (!target) {
      return "";
    }

    return buildSiblingPath(target.path, nextName.trim() || target.name);
  }, [nextName, target]);

  if (!target) {
    return null;
  }

  const handleSubmit = () => {
    const trimmedName = nextName.trim();
    const nextError = validateNextName(trimmedName);
    if (nextError) {
      setError(nextError);
      return;
    }

    setError(null);
    void onSubmit(trimmedName);
  };

  return (
    <div
      className="ai-dialog-overlay ai-file-rename-overlay"
      onClick={() => {
        if (!saving) {
          onClose();
        }
      }}
    >
      <div
        className="ai-dialog ai-file-rename-dialog"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !saving) {
            event.preventDefault();
            onClose();
          }

          if (event.key === "Enter") {
            event.preventDefault();
            handleSubmit();
          }
        }}
      >
        <div className="ai-dialog-header">
          <div className="ai-dialog-title-block">
            <div className="ai-dialog-title">重命名{target.isDir ? "目录" : "文件"}</div>
            <div className="ai-dialog-subtitle">
              {getRelativePathDisplay(projectPath, target.path)}
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            disabled={saving}
            className="ai-dialog-close"
            aria-label="关闭重命名弹窗"
          >
            <X size={16} />
          </button>
        </div>

        <div className="ai-dialog-body">
          <div className="ai-field">
            <label className="ai-field-label" htmlFor="file-explorer-rename-input">
              新名称
            </label>
            <input
              id="file-explorer-rename-input"
              ref={inputRef}
              value={nextName}
              onChange={(event) => {
                setNextName(event.target.value);
                if (error) {
                  setError(null);
                }
              }}
              disabled={saving}
              className="ai-field-input"
            />
          </div>

          <div className="ai-file-rename-preview">
            <div className="ai-file-rename-preview-label">重命名后路径</div>
            <div className="ai-file-rename-preview-path">
              {getRelativePathDisplay(projectPath, nextPathPreview)}
            </div>
          </div>

          {error && <div className="ai-dialog-error">{error}</div>}
        </div>

        <div className="ai-dialog-footer">
          <button type="button" onClick={onClose} disabled={saving} className="ai-button ai-button-ghost">
            取消
          </button>
          <button type="button" onClick={handleSubmit} disabled={saving} className="ai-button ai-button-primary">
            {saving ? "重命名中..." : "确认重命名"}
          </button>
        </div>
      </div>
    </div>
  );
}

export type { RenameTarget };
