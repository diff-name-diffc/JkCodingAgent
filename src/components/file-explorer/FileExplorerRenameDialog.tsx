import { useEffect, useMemo, useRef, useState } from "react";
import { X } from "lucide-react";
import s from "../../styles";
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
      style={s.modalOverlay}
      onClick={() => {
        if (!saving) {
          onClose();
        }
      }}
    >
      <div
        style={s.compactDialogBox}
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
        <div style={s.compactDialogHeader}>
          <div style={s.compactDialogTitleBlock}>
            <div style={s.compactDialogTitle}>重命名{target.isDir ? "目录" : "文件"}</div>
            <div style={s.compactDialogSubtitle}>
              {getRelativePathDisplay(projectPath, target.path)}
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            disabled={saving}
            style={s.modalCloseBtn}
            aria-label="关闭重命名弹窗"
          >
            <X size={16} />
          </button>
        </div>

        <div style={s.compactDialogBody}>
          <div style={s.modalField}>
            <label style={s.modalLabel} htmlFor="file-explorer-rename-input">
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
              style={s.modalInput}
            />
          </div>

          <div style={s.compactDialogPreview}>
            <div style={s.compactDialogPreviewLabel}>重命名后路径</div>
            <div style={s.compactDialogPreviewPath}>
              {getRelativePathDisplay(projectPath, nextPathPreview)}
            </div>
          </div>

          {error && <div style={s.compactDialogError}>{error}</div>}
        </div>

        <div style={s.compactDialogFooter}>
          <button type="button" onClick={onClose} disabled={saving} style={s.modalCancelBtn}>
            取消
          </button>
          <button type="button" onClick={handleSubmit} disabled={saving} style={s.modalSaveBtn}>
            {saving ? "重命名中..." : "确认重命名"}
          </button>
        </div>
      </div>
    </div>
  );
}

export type { RenameTarget };
