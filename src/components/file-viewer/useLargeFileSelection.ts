import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  focusAfterInsert,
  normalizeSelectionRange,
  selectedCharacterCount,
  selectedText,
} from "./large-file-selection";
import type {
  PendingFocus,
  RopeEditResult,
  SelectionPoint,
  SelectionRange,
} from "./large-file-types";
import { setCaretPosition } from "./useLargeFileViewport";

interface UseLargeFileSelectionOptions {
  sessionId: string;
  contentAreaRef: React.RefObject<HTMLDivElement | null>;
  lineCache: React.RefObject<Map<number, string>>;
  editingLineRef: React.RefObject<number | null>;
  charWidthRef: React.RefObject<number>;
  getLineElement: (line: number) => HTMLElement | null;
  flushActiveEdit: () => Promise<void>;
  readLines: (startLine: number, endLine: number) => Promise<string[]>;
  finishStructuralEdit: (
    result: RopeEditResult,
    invalidateFrom: number,
    focus: PendingFocus,
  ) => void;
  setEditingLine: (line: number | null) => void;
}

export function useLargeFileSelection({
  sessionId,
  contentAreaRef,
  lineCache,
  editingLineRef,
  charWidthRef,
  getLineElement,
  flushActiveEdit,
  readLines,
  finishStructuralEdit,
  setEditingLine,
}: UseLargeFileSelectionOptions) {
  const [selectionRange, setSelectionRange] = useState<SelectionRange | null>(null);
  const mouseSelectionRef = useRef<{
    active: boolean;
    dragging: boolean;
    anchor: SelectionPoint | null;
  }>({ active: false, dragging: false, anchor: null });

  const clearSelection = useCallback(() => {
    setSelectionRange(null);
    window.getSelection()?.removeAllRanges();
  }, []);

  const selectionPointFromCoordinates = useCallback(
    (clientX: number, clientY: number): SelectionPoint | null => {
      const contentArea = contentAreaRef.current;
      if (!contentArea) return null;
      const lineElement = document.elementFromPoint(clientX, clientY)?.closest("[data-line]");
      if (!(lineElement instanceof HTMLElement) || !contentArea.contains(lineElement)) return null;

      const line = Number(lineElement.dataset.line);
      if (Number.isNaN(line)) return null;
      const textLength = (lineCache.current.get(line) ?? lineElement.textContent ?? "").length;
      const relativeX = Math.max(0, clientX - lineElement.getBoundingClientRect().left - 8);
      return {
        line,
        col: Math.max(0, Math.min(textLength, Math.round(relativeX / charWidthRef.current))),
      };
    },
    [charWidthRef, contentAreaRef, lineCache],
  );

  const getSelectedText = useCallback(
    async (range: SelectionRange) => {
      const lines = await readLines(range.startLine, range.endLine);
      return lines.length === 0 ? "" : selectedText(lines, range);
    },
    [readLines],
  );

  const replaceSelection = useCallback(
    async (range: SelectionRange, insertText: string) => {
      await flushActiveEdit();
      editingLineRef.current = null;
      setEditingLine(null);
      const lines = await readLines(range.startLine, range.endLine);
      if (lines.length === 0) {
        clearSelection();
        return;
      }
      const result = await invoke<RopeEditResult>("rope_edit", {
        sessionId,
        line: range.startLine,
        col: range.startCol,
        deleteCount: selectedCharacterCount(lines, range),
        insertText,
      });
      clearSelection();
      finishStructuralEdit(
        result,
        range.startLine,
        focusAfterInsert(range.startLine, range.startCol, insertText),
      );
    },
    [
      clearSelection,
      editingLineRef,
      finishStructuralEdit,
      flushActiveEdit,
      readLines,
      sessionId,
      setEditingLine,
    ],
  );

  const handleMouseDown = useCallback(
    (event: React.MouseEvent<HTMLSpanElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      const point = selectionPointFromCoordinates(event.clientX, event.clientY);
      if (!point) {
        clearSelection();
        return;
      }
      editingLineRef.current = point.line;
      setEditingLine(point.line);
      clearSelection();
      event.currentTarget.focus();
      setCaretPosition(event.currentTarget, point.col);
      mouseSelectionRef.current = { active: true, dragging: false, anchor: point };
    },
    [clearSelection, editingLineRef, selectionPointFromCoordinates, setEditingLine],
  );

  const handleGlobalMouseMove = useCallback(
    (event: MouseEvent) => {
      const state = mouseSelectionRef.current;
      if (!state.active || !state.anchor) return;
      const current = selectionPointFromCoordinates(event.clientX, event.clientY);
      if (!current) return;
      if (
        !state.dragging &&
        current.line === state.anchor.line &&
        current.col === state.anchor.col
      ) {
        return;
      }
      state.dragging = true;
      editingLineRef.current = null;
      setEditingLine(null);
      window.getSelection()?.removeAllRanges();
      setSelectionRange(normalizeSelectionRange(state.anchor, current));
    },
    [editingLineRef, selectionPointFromCoordinates, setEditingLine],
  );

  const handleGlobalMouseUp = useCallback(() => {
    const state = mouseSelectionRef.current;
    if (state.active && state.anchor && !state.dragging) {
      const element = getLineElement(state.anchor.line);
      if (element) {
        editingLineRef.current = state.anchor.line;
        setEditingLine(state.anchor.line);
        element.focus();
        setCaretPosition(element, state.anchor.col);
      }
    }
    mouseSelectionRef.current = { active: false, dragging: false, anchor: null };
  }, [editingLineRef, getLineElement, setEditingLine]);

  useEffect(() => {
    window.addEventListener("mousemove", handleGlobalMouseMove);
    window.addEventListener("mouseup", handleGlobalMouseUp);
    return () => {
      window.removeEventListener("mousemove", handleGlobalMouseMove);
      window.removeEventListener("mouseup", handleGlobalMouseUp);
    };
  }, [handleGlobalMouseMove, handleGlobalMouseUp]);

  return {
    selectionRange,
    mouseSelectionRef,
    clearSelection,
    getSelectedText,
    replaceSelection,
    handleMouseDown,
  };
}
