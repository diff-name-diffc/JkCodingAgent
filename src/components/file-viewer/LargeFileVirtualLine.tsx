import { memo } from "react";
import type { RefObject } from "react";
import { LARGE_FILE_LINE_HEIGHT, type SelectionRange } from "./large-file-types";

interface LargeFileVirtualLineProps {
  idx: number;
  text: string;
  isEditing: boolean;
  selectionRange: SelectionRange | null;
  gutterWidth: number;
  charWidth: number;
  onMouseDown: (event: React.MouseEvent<HTMLSpanElement>) => void;
  onFocus: (lineIdx: number) => void;
  onBlur: (lineIdx: number, element: HTMLElement) => void;
  onInput: (lineIdx: number, element: HTMLElement) => void;
  onKeyDown: (event: React.KeyboardEvent<HTMLSpanElement>, lineIdx: number) => void;
  onPaste: (event: React.ClipboardEvent<HTMLSpanElement>, lineIdx: number) => void;
  editingLineRef: RefObject<number | null>;
}

export const LargeFileVirtualLine = memo(function LargeFileVirtualLine({
  idx,
  text,
  isEditing,
  selectionRange,
  gutterWidth,
  charWidth,
  onMouseDown,
  onFocus,
  onBlur,
  onInput,
  onKeyDown,
  onPaste,
  editingLineRef,
}: LargeFileVirtualLineProps) {
  let overlayStyle: { left: number; width: number } | null = null;
  if (selectionRange && idx >= selectionRange.startLine && idx <= selectionRange.endLine) {
    const startCol = idx === selectionRange.startLine ? selectionRange.startCol : 0;
    const endCol = idx === selectionRange.endLine ? selectionRange.endCol : text.length;
    const widthChars = Math.max(endCol - startCol, 0);
    if (widthChars > 0) {
      overlayStyle = {
        left: gutterWidth + 8 + startCol * charWidth,
        width: Math.max(widthChars * charWidth, 2),
      };
    }
  }

  return (
    <div
      className="ai-large-file-line"
      style={{ position: "absolute", top: idx * LARGE_FILE_LINE_HEIGHT }}
    >
      {overlayStyle && <div className="ai-large-file-selection" style={overlayStyle} />}
      <span
        className={isEditing ? "ai-large-file-gutter is-editing" : "ai-large-file-gutter"}
        style={{ width: gutterWidth }}
      >
        {idx + 1}
      </span>
      <span
        data-line={idx}
        contentEditable
        suppressContentEditableWarning
        spellCheck={false}
        ref={(element) => {
          if (element && editingLineRef.current !== idx && element.textContent !== text) {
            element.textContent = text;
          }
        }}
        onMouseDown={onMouseDown}
        onFocus={() => onFocus(idx)}
        onBlur={(event) => onBlur(idx, event.currentTarget)}
        onInput={(event) => onInput(idx, event.currentTarget)}
        onKeyDown={(event) => onKeyDown(event, idx)}
        onPaste={(event) => onPaste(event, idx)}
        className={[
          "ai-large-file-content",
          isEditing ? "is-editing" : "",
          selectionRange ? "has-selection" : "",
        ]
          .filter(Boolean)
          .join(" ")}
      />
    </div>
  );
});
