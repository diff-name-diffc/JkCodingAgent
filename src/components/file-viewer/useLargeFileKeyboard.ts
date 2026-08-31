import { useCallback, useEffect } from "react";
import { isImeComposing } from "../../utils";
import type { SelectionRange } from "./large-file-types";
import { getCaretOffset, setCaretPosition } from "./useLargeFileViewport";

interface UseLargeFileKeyboardOptions {
  active: boolean;
  totalLines: number;
  selectionRange: SelectionRange | null;
  editingLineRef: React.RefObject<number | null>;
  mouseSelectionRef: React.RefObject<{ dragging: boolean }>;
  getLineElement: (line: number) => HTMLElement | null;
  setEditingLine: (line: number | null) => void;
  clearSelection: () => void;
  getSelectedText: (range: SelectionRange) => Promise<string>;
  replaceSelection: (range: SelectionRange, text: string) => Promise<void>;
  commitLineEdit: (line: number, content: string) => void;
  editLineInput: (line: number, element: HTMLElement) => void;
  insertTextAtCursor: (line: number, col: number, text: string) => Promise<void>;
  mergeWithPreviousLine: (line: number, content: string) => Promise<void>;
  mergeWithNextLine: (line: number, content: string) => Promise<void>;
  flushActiveEdit: () => Promise<void>;
  handleUndoRedo: (redo: boolean) => Promise<void>;
  save: () => Promise<void>;
}

export function useLargeFileKeyboard({
  active,
  totalLines,
  selectionRange,
  editingLineRef,
  mouseSelectionRef,
  getLineElement,
  setEditingLine,
  clearSelection,
  getSelectedText,
  replaceSelection,
  commitLineEdit,
  editLineInput,
  insertTextAtCursor,
  mergeWithPreviousLine,
  mergeWithNextLine,
  flushActiveEdit,
  handleUndoRedo,
  save,
}: UseLargeFileKeyboardOptions) {
  const handleInput = useCallback(
    (line: number, element: HTMLElement) => {
      if (selectionRange) clearSelection();
      editLineInput(line, element);
    },
    [clearSelection, editLineInput, selectionRange],
  );

  const moveVertically = useCallback(
    (line: number, delta: -1 | 1, element: HTMLElement) => {
      const caretCol = getCaretOffset();
      commitLineEdit(line, element.textContent ?? "");
      editingLineRef.current = null;
      setEditingLine(null);
      const target = getLineElement(line + delta);
      if (target) {
        editingLineRef.current = line + delta;
        setEditingLine(line + delta);
        target.focus();
        setCaretPosition(target, caretCol);
      }
    },
    [commitLineEdit, editingLineRef, getLineElement, setEditingLine],
  );

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLSpanElement>, line: number) => {
      if (isImeComposing(event)) return;
      if (selectionRange) {
        let replacement: string | null = null;
        if (event.key === "Backspace" || event.key === "Delete") replacement = "";
        else if (event.key === "Enter") replacement = "\n";
        else if (event.key === "Tab") replacement = "  ";
        else if (!event.metaKey && !event.ctrlKey && !event.altKey && event.key.length === 1) {
          replacement = event.key;
        }
        if (replacement !== null) {
          event.preventDefault();
          void replaceSelection(selectionRange, replacement);
          return;
        }
      }

      if (event.key === "ArrowUp" || event.key === "ArrowDown") {
        event.preventDefault();
        moveVertically(line, event.key === "ArrowUp" ? -1 : 1, event.currentTarget);
      } else if (event.key === "Enter") {
        event.preventDefault();
        void insertTextAtCursor(line, getCaretOffset(), "\n");
      } else if (event.key === "Backspace" && getCaretOffset() === 0) {
        event.preventDefault();
        if (line > 0) void mergeWithPreviousLine(line, event.currentTarget.textContent ?? "");
      } else if (event.key === "Delete") {
        const content = event.currentTarget.textContent ?? "";
        if (getCaretOffset() === content.length && line < totalLines - 1) {
          event.preventDefault();
          void mergeWithNextLine(line, content);
        }
      } else if (event.key === "Tab") {
        event.preventDefault();
        const col = getCaretOffset();
        const element = event.currentTarget;
        const text = element.textContent ?? "";
        element.textContent = `${text.slice(0, col)}  ${text.slice(col)}`;
        setCaretPosition(element, col + 2);
        handleInput(line, element);
      }
    },
    [
      handleInput,
      insertTextAtCursor,
      mergeWithNextLine,
      mergeWithPreviousLine,
      moveVertically,
      replaceSelection,
      selectionRange,
      totalLines,
    ],
  );

  const handlePaste = useCallback(
    (event: React.ClipboardEvent<HTMLSpanElement>, line: number) => {
      event.preventDefault();
      const pastedText = event.clipboardData.getData("text/plain");
      if (!pastedText) return;
      if (selectionRange) {
        void replaceSelection(selectionRange, pastedText);
        return;
      }
      const col = getCaretOffset();
      if (pastedText.includes("\n")) {
        void insertTextAtCursor(line, col, pastedText);
        return;
      }
      const element = event.currentTarget;
      const text = element.textContent ?? "";
      element.textContent = `${text.slice(0, col)}${pastedText}${text.slice(col)}`;
      setCaretPosition(element, col + pastedText.length);
      handleInput(line, element);
    },
    [handleInput, insertTextAtCursor, replaceSelection, selectionRange],
  );

  const handleFocus = useCallback(
    (line: number) => {
      if (mouseSelectionRef.current.dragging) return;
      editingLineRef.current = line;
      setEditingLine(line);
    },
    [editingLineRef, mouseSelectionRef, setEditingLine],
  );

  const handleBlur = useCallback(
    (line: number, element: HTMLElement) => {
      commitLineEdit(line, element.textContent ?? "");
      editingLineRef.current = null;
      setEditingLine(null);
    },
    [commitLineEdit, editingLineRef, setEditingLine],
  );

  useEffect(() => {
    if (!active) return;
    const onKeyDown = (event: KeyboardEvent) => {
      const modifier = event.metaKey || event.ctrlKey;
      const key = event.key.toLowerCase();
      if (modifier && key === "c" && selectionRange) {
        event.preventDefault();
        void flushActiveEdit()
          .then(() => getSelectedText(selectionRange))
          .then((text) => navigator.clipboard.writeText(text));
      } else if (modifier && key === "x" && selectionRange) {
        event.preventDefault();
        void flushActiveEdit()
          .then(() => getSelectedText(selectionRange))
          .then((text) => navigator.clipboard.writeText(text))
          .then(() => replaceSelection(selectionRange, ""));
      } else if (event.key === "Escape" && selectionRange) {
        event.preventDefault();
        clearSelection();
      } else if (modifier && key === "s") {
        event.preventDefault();
        void save().catch((error) => console.error("rope_save failed:", error));
      } else if (modifier && key === "z") {
        event.preventDefault();
        void handleUndoRedo(event.shiftKey);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [
    active,
    clearSelection,
    flushActiveEdit,
    getSelectedText,
    handleUndoRedo,
    replaceSelection,
    save,
    selectionRange,
  ]);

  return { handleInput, handleKeyDown, handlePaste, handleFocus, handleBlur };
}
