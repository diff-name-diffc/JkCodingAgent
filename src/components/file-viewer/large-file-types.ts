import type { RefObject } from "react";

export const LARGE_FILE_LINE_HEIGHT = 22;
export const LARGE_FILE_OVERSCAN = 40;
export const LARGE_FILE_CHUNK_SIZE = 200;

export interface FileMeta {
  sizeBytes: number;
  lineCount: number;
  isText: boolean;
}

export interface RopeMeta {
  lineCount: number;
  charCount: number;
  byteLen: number;
}

export interface RopeEditResult {
  lineCount: number;
  affectedStartLine: number;
  affectedEndLine: number;
}

export interface SelectionPoint {
  line: number;
  col: number;
}

export interface SelectionRange {
  startLine: number;
  startCol: number;
  endLine: number;
  endCol: number;
}

export interface PendingFocus {
  line: number;
  col: number;
}

export interface LargeFileRefs {
  contentAreaRef: RefObject<HTMLDivElement | null>;
  lineCache: RefObject<Map<number, string>>;
  syncedLineCache: RefObject<Map<number, string>>;
  editingLineRef: RefObject<number | null>;
  pendingFocusRef: RefObject<PendingFocus | null>;
  charWidthRef: RefObject<number>;
}
