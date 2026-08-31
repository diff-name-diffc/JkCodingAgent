import type { SelectionPoint, SelectionRange } from "./large-file-types";

export function compareSelectionPoints(a: SelectionPoint, b: SelectionPoint): number {
  return a.line === b.line ? a.col - b.col : a.line - b.line;
}

export function normalizeSelectionRange(
  anchor: SelectionPoint,
  current: SelectionPoint,
): SelectionRange {
  const [start, end] =
    compareSelectionPoints(anchor, current) <= 0 ? [anchor, current] : [current, anchor];
  return {
    startLine: start.line,
    startCol: start.col,
    endLine: end.line,
    endCol: end.col,
  };
}

export function selectedText(lines: string[], range: SelectionRange): string {
  return lines
    .map((line, index) => {
      if (range.startLine === range.endLine) return line.slice(range.startCol, range.endCol);
      if (index === 0) return line.slice(range.startCol);
      if (index === lines.length - 1) return line.slice(0, range.endCol);
      return line;
    })
    .join("\n");
}

export function selectedCharacterCount(lines: string[], range: SelectionRange): number {
  if (range.startLine === range.endLine) return range.endCol - range.startCol;
  return lines.reduce((total, line, index) => {
    if (index === 0) return total + line.length - range.startCol + 1;
    if (index === lines.length - 1) return total + range.endCol;
    return total + line.length + 1;
  }, 0);
}

export function focusAfterInsert(line: number, col: number, text: string): SelectionPoint {
  const newlineCount = (text.match(/\n/g) ?? []).length;
  const lastNewline = text.lastIndexOf("\n");
  return {
    line: line + newlineCount,
    col: lastNewline >= 0 ? text.length - lastNewline - 1 : col + text.length,
  };
}
