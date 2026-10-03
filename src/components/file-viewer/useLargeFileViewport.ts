import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  LARGE_FILE_CHUNK_SIZE,
  LARGE_FILE_LINE_HEIGHT,
  LARGE_FILE_OVERSCAN,
  type RopeMeta,
} from "./large-file-types";

interface UseLargeFileViewportOptions {
  active: boolean;
  sessionId: string;
  filePath: string;
  projectPath: string;
  initialLineCount: number;
}

/** 大文件只读视口：按需分块经 `rope_read_lines` 拉取行文本，虚拟滚动渲染。 */
export function useLargeFileViewport({
  active,
  sessionId,
  filePath,
  projectPath,
  initialLineCount,
}: UseLargeFileViewportOptions) {
  const containerRef = useRef<HTMLDivElement>(null);
  const lineCache = useRef(new Map<number, string>());
  const pendingFetches = useRef(new Set<string>());
  const [visibleRange, setVisibleRange] = useState({ start: 0, end: 100 });
  const [renderedLines, setRenderedLines] = useState<{ idx: number; text: string }[]>([]);
  const [ropeReady, setRopeReady] = useState(false);
  const [totalLines, setTotalLines] = useState(initialLineCount);

  useEffect(() => {
    let cancelled = false;
    lineCache.current.clear();
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
          lines.forEach((text, offset) => lineCache.current.set(chunk.start + offset, text));
          pendingFetches.current.delete(chunk.key);
        }
      }
      updateRenderedLines(start, end);
    },
    [ropeReady, sessionId, totalLines, updateRenderedLines],
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

  return { containerRef, renderedLines, totalLines, handleScroll };
}
