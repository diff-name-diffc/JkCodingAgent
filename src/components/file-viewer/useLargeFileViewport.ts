import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  LARGE_FILE_CHUNK_SIZE,
  LARGE_FILE_LINE_HEIGHT,
  LARGE_FILE_OVERSCAN,
  type PendingFocus,
  type RopeMeta,
} from "./large-file-types";

interface UseLargeFileViewportOptions {
  active: boolean;
  sessionId: string;
  filePath: string;
  projectPath: string;
  initialLineCount: number;
  editingLineRef: React.RefObject<number | null>;
  pendingFocusRef: React.RefObject<PendingFocus | null>;
}

export function useLargeFileViewport({
  active,
  sessionId,
  filePath,
  projectPath,
  initialLineCount,
  editingLineRef,
  pendingFocusRef,
}: UseLargeFileViewportOptions) {
  const containerRef = useRef<HTMLDivElement>(null);
  const contentAreaRef = useRef<HTMLDivElement>(null);
  const lineCache = useRef(new Map<number, string>());
  const syncedLineCache = useRef(new Map<number, string>());
  const pendingFetches = useRef(new Set<string>());
  const [visibleRange, setVisibleRange] = useState({ start: 0, end: 100 });
  const [renderedLines, setRenderedLines] = useState<{ idx: number; text: string }[]>([]);
  const [ropeReady, setRopeReady] = useState(false);
  const [totalLines, setTotalLines] = useState(initialLineCount);

  useEffect(() => {
    let cancelled = false;
    lineCache.current.clear();
    syncedLineCache.current.clear();
    pendingFetches.current.clear();
    setRenderedLines([]);
    setRopeReady(false);
    setVisibleRange({ start: 0, end: 100 });

    invoke<RopeMeta>("rope_open", { sessionId, path: filePath, projectPath })
      .then((meta) => {
        if (!cancelled) {
          setTotalLines(meta.lineCount);
          setRopeReady(true);
        }
      })
      .catch((error) => console.error("Failed to open rope:", error));

    return () => {
      cancelled = true;
      invoke("rope_close", { sessionId }).catch(() => {});
    };
  }, [filePath, projectPath, sessionId]);

  const updateRenderedLines = useCallback(
    (start: number, end: number) => {
      const rangeStart = Math.max(0, start - LARGE_FILE_OVERSCAN);
      const rangeEnd = Math.min(totalLines, end + LARGE_FILE_OVERSCAN);
      const lines = Array.from({ length: rangeEnd - rangeStart }, (_, offset) => {
        const idx = rangeStart + offset;
        return { idx, text: lineCache.current.get(idx) ?? "" };
      });
      setRenderedLines(lines);
    },
    [totalLines],
  );

  const loadRange = useCallback(
    async (start: number, end: number) => {
      if (!ropeReady) return;

      const rangeStart = Math.max(0, start - LARGE_FILE_OVERSCAN);
      const rangeEnd = Math.min(totalLines, end + LARGE_FILE_OVERSCAN);
      const chunks: { start: number; end: number; key: string }[] = [];
      for (let line = rangeStart; line < rangeEnd; line++) {
        if (!lineCache.current.has(line)) {
          const chunkStart = Math.floor(line / LARGE_FILE_CHUNK_SIZE) * LARGE_FILE_CHUNK_SIZE;
          const chunkEnd = Math.min(chunkStart + LARGE_FILE_CHUNK_SIZE, totalLines);
          const key = `${chunkStart}-${chunkEnd}`;
          if (!pendingFetches.current.has(key)) {
            pendingFetches.current.add(key);
            chunks.push({ start: chunkStart, end: chunkEnd, key });
          }
          line = chunkEnd - 1;
        }
      }

      if (chunks.length > 0) {
        const results = await Promise.all(
          chunks.map(async (chunk) => ({
            chunk,
            lines: await invoke<string[]>("rope_read_lines", {
              sessionId,
              startLine: chunk.start,
              maxLines: chunk.end - chunk.start,
            }),
          })),
        );
        for (const { chunk, lines } of results) {
          lines.forEach((text, offset) => {
            const line = chunk.start + offset;
            if (editingLineRef.current !== line) {
              lineCache.current.set(line, text);
              syncedLineCache.current.set(line, text);
            }
          });
          pendingFetches.current.delete(chunk.key);
        }
      }
      updateRenderedLines(start, end);
    },
    [editingLineRef, ropeReady, sessionId, totalLines, updateRenderedLines],
  );

  const handleScroll = useCallback(() => {
    const container = containerRef.current;
    if (!container) return;
    setVisibleRange({
      start: Math.floor(container.scrollTop / LARGE_FILE_LINE_HEIGHT),
      end: Math.ceil((container.scrollTop + container.clientHeight) / LARGE_FILE_LINE_HEIGHT),
    });
  }, []);

  useEffect(() => {
    void loadRange(visibleRange.start, visibleRange.end);
  }, [loadRange, visibleRange]);

  useEffect(() => {
    if (!active) return;
    const frame = requestAnimationFrame(handleScroll);
    return () => cancelAnimationFrame(frame);
  }, [active, handleScroll]);

  useEffect(() => {
    const target = pendingFocusRef.current;
    if (!target) return;
    const frame = requestAnimationFrame(() => {
      const element = contentAreaRef.current?.querySelector(
        `[data-line="${target.line}"]`,
      ) as HTMLElement | null;
      if (element) {
        editingLineRef.current = target.line;
        element.focus();
        setCaretPosition(element, target.col);
      }
      pendingFocusRef.current = null;
    });
    return () => cancelAnimationFrame(frame);
  }, [editingLineRef, pendingFocusRef, renderedLines]);

  const getLineElement = useCallback((line: number) => {
    return contentAreaRef.current?.querySelector(`[data-line="${line}"]`) as HTMLElement | null;
  }, []);

  const invalidateCacheFrom = useCallback((fromLine: number) => {
    lineCache.current = new Map([...lineCache.current].filter(([line]) => line < fromLine));
    syncedLineCache.current = new Map(
      [...syncedLineCache.current].filter(([line]) => line < fromLine),
    );
    for (const key of pendingFetches.current) {
      if (Number(key.split("-")[1]) > fromLine) pendingFetches.current.delete(key);
    }
  }, []);

  const clearCache = useCallback(() => {
    lineCache.current.clear();
    syncedLineCache.current.clear();
    pendingFetches.current.clear();
  }, []);

  return {
    containerRef,
    contentAreaRef,
    lineCache,
    syncedLineCache,
    visibleRange,
    renderedLines,
    totalLines,
    setTotalLines,
    handleScroll,
    getLineElement,
    invalidateCacheFrom,
    clearCache,
    loadRange,
  };
}

export function setCaretPosition(element: HTMLElement, offset: number) {
  const textNode = element.firstChild;
  if (!textNode) {
    element.focus();
    return;
  }
  const range = document.createRange();
  range.setStart(textNode, Math.min(offset, textNode.textContent?.length ?? 0));
  range.collapse(true);
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

export function getCaretOffset(): number {
  return window.getSelection()?.focusOffset ?? 0;
}
