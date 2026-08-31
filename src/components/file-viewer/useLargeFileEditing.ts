import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { focusAfterInsert } from "./large-file-selection";
import type { PendingFocus, RopeEditResult, RopeMeta } from "./large-file-types";

interface UseLargeFileEditingOptions {
  sessionId: string;
  filePath: string;
  projectPath: string;
  totalLines: number;
  visibleRange: { start: number; end: number };
  lineCache: React.RefObject<Map<number, string>>;
  syncedLineCache: React.RefObject<Map<number, string>>;
  editingLineRef: React.RefObject<number | null>;
  pendingFocusRef: React.RefObject<PendingFocus | null>;
  getLineElement: (line: number) => HTMLElement | null;
  setTotalLines: (lineCount: number) => void;
  invalidateCacheFrom: (line: number) => void;
  clearCache: () => void;
  loadRange: (start: number, end: number) => Promise<void>;
  setEditingLine: (line: number | null) => void;
  onDirtyChange?: (dirty: boolean) => void;
}

export function useLargeFileEditing({
  sessionId,
  filePath,
  projectPath,
  totalLines,
  visibleRange,
  lineCache,
  syncedLineCache,
  editingLineRef,
  pendingFocusRef,
  getLineElement,
  setTotalLines,
  invalidateCacheFrom,
  clearCache,
  loadRange,
  setEditingLine,
  onDirtyChange,
}: UseLargeFileEditingOptions) {
  const [dirty, setDirty] = useState(false);
  const editTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingLocalEditRef = useRef<{ line: number; content: string } | null>(null);

  useEffect(() => {
    setDirty(false);
  }, [filePath, sessionId]);

  const markDirty = useCallback(() => {
    setDirty(true);
    onDirtyChange?.(true);
  }, [onDirtyChange]);

  const flushPendingEdit = useCallback(
    async (line: number, content: string) => {
      if (editTimerRef.current) clearTimeout(editTimerRef.current);
      editTimerRef.current = null;
      pendingLocalEditRef.current = null;
      if ((syncedLineCache.current.get(line) ?? "") === content) return;

      await invoke<RopeEditResult>("rope_replace_line", {
        sessionId,
        line,
        newContent: content,
      });
      syncedLineCache.current.set(line, content);
    },
    [sessionId, syncedLineCache],
  );

  const flushActiveEdit = useCallback(async () => {
    if (editTimerRef.current) clearTimeout(editTimerRef.current);
    editTimerRef.current = null;
    const pending = pendingLocalEditRef.current;
    if (pending) {
      await flushPendingEdit(pending.line, pending.content);
      return;
    }
    const editingLine = editingLineRef.current;
    const element = editingLine === null ? null : getLineElement(editingLine);
    if (editingLine !== null && element) {
      await flushPendingEdit(editingLine, element.textContent ?? "");
    }
  }, [editingLineRef, flushPendingEdit, getLineElement]);

  const readLines = useCallback(
    (startLine: number, endLine: number) =>
      invoke<string[]>("rope_read_lines", {
        sessionId,
        startLine,
        maxLines: endLine - startLine + 1,
      }),
    [sessionId],
  );

  const commitLineEdit = useCallback(
    (line: number, content: string) => {
      if (editTimerRef.current) clearTimeout(editTimerRef.current);
      editTimerRef.current = null;
      const previous = syncedLineCache.current.get(line) ?? "";
      if (previous === content) {
        pendingLocalEditRef.current = null;
        return;
      }
      pendingLocalEditRef.current = null;
      lineCache.current.set(line, content);
      markDirty();
      invoke<RopeEditResult>("rope_replace_line", { sessionId, line, newContent: content })
        .then((result) => {
          syncedLineCache.current.set(line, content);
          if (result.lineCount !== totalLines) setTotalLines(result.lineCount);
        })
        .catch((error) => {
          console.error("rope_replace_line failed:", error);
          lineCache.current.set(line, previous);
          syncedLineCache.current.set(line, previous);
        });
    },
    [lineCache, markDirty, sessionId, setTotalLines, syncedLineCache, totalLines],
  );

  const handleInput = useCallback(
    (line: number, element: HTMLElement) => {
      const content = element.textContent ?? "";
      lineCache.current.set(line, content);
      pendingLocalEditRef.current = { line, content };
      if (editTimerRef.current) clearTimeout(editTimerRef.current);
      editTimerRef.current = setTimeout(() => commitLineEdit(line, content), 150);
      markDirty();
    },
    [commitLineEdit, lineCache, markDirty],
  );

  const finishStructuralEdit = useCallback(
    (result: RopeEditResult, invalidateFrom: number, focus: PendingFocus) => {
      markDirty();
      invalidateCacheFrom(invalidateFrom);
      setTotalLines(result.lineCount);
      pendingFocusRef.current = focus;
    },
    [invalidateCacheFrom, markDirty, pendingFocusRef, setTotalLines],
  );

  const insertTextAtCursor = useCallback(
    async (line: number, col: number, text: string) => {
      const element = getLineElement(line);
      await flushPendingEdit(line, element?.textContent ?? lineCache.current.get(line) ?? "");
      editingLineRef.current = null;
      setEditingLine(null);
      const result = await invoke<RopeEditResult>("rope_edit", {
        sessionId,
        line,
        col,
        deleteCount: 0,
        insertText: text,
      });
      finishStructuralEdit(result, line, focusAfterInsert(line, col, text));
    },
    [
      editingLineRef,
      finishStructuralEdit,
      flushPendingEdit,
      getLineElement,
      lineCache,
      sessionId,
      setEditingLine,
    ],
  );

  const mergeWithPreviousLine = useCallback(
    async (line: number, currentContent: string) => {
      const previousContent = lineCache.current.get(line - 1) ?? "";
      await flushPendingEdit(line, currentContent);
      editingLineRef.current = null;
      setEditingLine(null);
      const result = await invoke<RopeEditResult>("rope_edit", {
        sessionId,
        line: line - 1,
        col: previousContent.length,
        deleteCount: 1,
        insertText: "",
      });
      finishStructuralEdit(result, line - 1, { line: line - 1, col: previousContent.length });
    },
    [editingLineRef, finishStructuralEdit, flushPendingEdit, lineCache, sessionId, setEditingLine],
  );

  const mergeWithNextLine = useCallback(
    async (line: number, content: string) => {
      await flushPendingEdit(line, content);
      editingLineRef.current = null;
      setEditingLine(null);
      const result = await invoke<RopeEditResult>("rope_edit", {
        sessionId,
        line,
        col: content.length,
        deleteCount: 1,
        insertText: "",
      });
      finishStructuralEdit(result, line, { line, col: content.length });
    },
    [editingLineRef, finishStructuralEdit, flushPendingEdit, sessionId, setEditingLine],
  );

  const handleUndoRedo = useCallback(
    async (redo: boolean) => {
      let focus: PendingFocus | null = null;
      const editingLine = editingLineRef.current;
      if (editingLine !== null) {
        const element = getLineElement(editingLine);
        focus = { line: editingLine, col: getCaretOffset() };
        if (element) await flushPendingEdit(editingLine, element.textContent ?? "");
        editingLineRef.current = null;
        setEditingLine(null);
      }
      try {
        const result = await invoke<RopeMeta>(redo ? "rope_redo" : "rope_undo", { sessionId });
        clearCache();
        setTotalLines(result.lineCount);
        markDirty();
        if (focus) {
          pendingFocusRef.current = {
            line: Math.min(focus.line, Math.max(result.lineCount - 1, 0)),
            col: focus.col,
          };
        }
        await loadRange(visibleRange.start, visibleRange.end);
      } catch {
        // Rope reports an error when its undo/redo stack is empty.
      }
    },
    [
      clearCache,
      editingLineRef,
      flushPendingEdit,
      getLineElement,
      loadRange,
      markDirty,
      pendingFocusRef,
      sessionId,
      setEditingLine,
      setTotalLines,
      visibleRange,
    ],
  );

  const save = useCallback(async () => {
    if (!dirty) return;
    await flushActiveEdit();
    await invoke("rope_save", { sessionId, projectPath });
    setDirty(false);
    onDirtyChange?.(false);
  }, [dirty, flushActiveEdit, onDirtyChange, projectPath, sessionId]);

  useEffect(
    () => () => {
      if (editTimerRef.current) clearTimeout(editTimerRef.current);
    },
    [],
  );

  return {
    dirty,
    markDirty,
    flushActiveEdit,
    readLines,
    commitLineEdit,
    handleInput,
    insertTextAtCursor,
    mergeWithPreviousLine,
    mergeWithNextLine,
    finishStructuralEdit,
    handleUndoRedo,
    save,
  };
}

function getCaretOffset(): number {
  return window.getSelection()?.focusOffset ?? 0;
}
