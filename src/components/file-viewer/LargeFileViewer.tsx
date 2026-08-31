import { useEffect, useMemo, useRef, useState } from "react";
import { LargeFileVirtualLine } from "./LargeFileVirtualLine";
import { LARGE_FILE_LINE_HEIGHT, type FileMeta, type PendingFocus } from "./large-file-types";
import { useLargeFileEditing } from "./useLargeFileEditing";
import { useLargeFileKeyboard } from "./useLargeFileKeyboard";
import { useLargeFileSelection } from "./useLargeFileSelection";
import { useLargeFileViewport } from "./useLargeFileViewport";

interface LargeFileViewerProps {
  active: boolean;
  sessionId: string;
  filePath: string;
  projectPath: string;
  meta: FileMeta;
  onDirtyChange?: (dirty: boolean) => void;
}

/** Virtual-scrolling text editor backed by the Rust rope session. */
export function LargeFileViewer({
  active,
  sessionId,
  filePath,
  projectPath,
  meta,
  onDirtyChange,
}: LargeFileViewerProps) {
  const editingLineRef = useRef<number | null>(null);
  const pendingFocusRef = useRef<PendingFocus | null>(null);
  const charWidthRef = useRef(7.8);
  const [editingLine, setEditingLine] = useState<number | null>(null);

  const viewport = useLargeFileViewport({
    active,
    sessionId,
    filePath,
    projectPath,
    initialLineCount: meta.lineCount,
    editingLineRef,
    pendingFocusRef,
  });

  const editing = useLargeFileEditing({
    sessionId,
    filePath,
    projectPath,
    totalLines: viewport.totalLines,
    visibleRange: viewport.visibleRange,
    lineCache: viewport.lineCache,
    syncedLineCache: viewport.syncedLineCache,
    editingLineRef,
    pendingFocusRef,
    getLineElement: viewport.getLineElement,
    setTotalLines: viewport.setTotalLines,
    invalidateCacheFrom: viewport.invalidateCacheFrom,
    clearCache: viewport.clearCache,
    loadRange: viewport.loadRange,
    setEditingLine,
    onDirtyChange,
  });

  const selection = useLargeFileSelection({
    sessionId,
    contentAreaRef: viewport.contentAreaRef,
    lineCache: viewport.lineCache,
    editingLineRef,
    charWidthRef,
    getLineElement: viewport.getLineElement,
    flushActiveEdit: editing.flushActiveEdit,
    readLines: editing.readLines,
    finishStructuralEdit: editing.finishStructuralEdit,
    setEditingLine,
  });

  const keyboard = useLargeFileKeyboard({
    active,
    totalLines: viewport.totalLines,
    selectionRange: selection.selectionRange,
    editingLineRef,
    mouseSelectionRef: selection.mouseSelectionRef,
    getLineElement: viewport.getLineElement,
    setEditingLine,
    clearSelection: selection.clearSelection,
    getSelectedText: selection.getSelectedText,
    replaceSelection: selection.replaceSelection,
    commitLineEdit: editing.commitLineEdit,
    editLineInput: editing.handleInput,
    insertTextAtCursor: editing.insertTextAtCursor,
    mergeWithPreviousLine: editing.mergeWithPreviousLine,
    mergeWithNextLine: editing.mergeWithNextLine,
    flushActiveEdit: editing.flushActiveEdit,
    handleUndoRedo: editing.handleUndoRedo,
    save: editing.save,
  });

  useEffect(() => {
    const probe = document.createElement("span");
    probe.textContent = "MMMMMMMMMM";
    Object.assign(probe.style, {
      position: "absolute",
      visibility: "hidden",
      pointerEvents: "none",
      fontFamily: "JetBrains Mono, monospace",
      fontSize: "13px",
      whiteSpace: "pre",
    });
    document.body.appendChild(probe);
    charWidthRef.current = probe.getBoundingClientRect().width / 10 || charWidthRef.current;
    document.body.removeChild(probe);
  }, []);

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
        <span
          className={
            editing.dirty ? "ai-large-file-status is-dirty" : "ai-large-file-status is-saved"
          }
        >
          {editing.dirty ? "已修改" : "已保存"}
        </span>
        <span>{sizeLabel}</span>
        <span>·</span>
        <span>{viewport.totalLines.toLocaleString()} 行</span>
        {editing.dirty && <span className="ai-large-file-save-hint">⌘S 保存</span>}
      </div>

      <div
        ref={viewport.containerRef}
        onScroll={viewport.handleScroll}
        tabIndex={-1}
        className="ai-large-file-scroll chat-scroll"
        style={{ lineHeight: `${LARGE_FILE_LINE_HEIGHT}px` }}
      >
        <div
          ref={viewport.contentAreaRef}
          className="ai-large-file-content-area"
          style={{ height: viewport.totalLines * LARGE_FILE_LINE_HEIGHT }}
        >
          {viewport.renderedLines.map(({ idx, text }) => (
            <LargeFileVirtualLine
              key={idx}
              idx={idx}
              text={text}
              isEditing={editingLine === idx}
              selectionRange={selection.selectionRange}
              gutterWidth={gutterWidth}
              charWidth={charWidthRef.current}
              onMouseDown={selection.handleMouseDown}
              onFocus={keyboard.handleFocus}
              onBlur={keyboard.handleBlur}
              onInput={keyboard.handleInput}
              onKeyDown={keyboard.handleKeyDown}
              onPaste={keyboard.handlePaste}
              editingLineRef={editingLineRef}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
