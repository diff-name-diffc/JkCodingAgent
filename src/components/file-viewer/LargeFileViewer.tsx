import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

const LINE_HEIGHT = 20;
const OVERSCAN = 40; // Extra lines above/below viewport
const CHUNK_SIZE = 200; // Lines per backend fetch

type FileMeta = {
  sizeBytes: number;
  lineCount: number;
  isText: boolean;
};

/**
 * Virtual-scrolling read-only text viewer for large files (2MB+).
 * Only renders lines visible in the viewport + overscan buffer.
 * Fetches line data in chunks from the backend on demand.
 */
export function LargeFileViewer({
  filePath,
  projectPath,
  meta,
}: {
  filePath: string;
  projectPath: string;
  meta: FileMeta;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [visibleRange, setVisibleRange] = useState({ start: 0, end: 100 });
  const lineCache = useRef<Map<number, string>>(new Map());
  const [renderedLines, setRenderedLines] = useState<{ idx: number; text: string }[]>([]);
  const pendingFetches = useRef<Set<string>>(new Set());

  const totalHeight = meta.lineCount * LINE_HEIGHT;

  // Compute which chunks need to be fetched for the visible range
  const fetchChunksForRange = useCallback(
    (start: number, end: number) => {
      const needed: { chunkStart: number; chunkEnd: number }[] = [];

      const rangeStart = Math.max(0, start - OVERSCAN);
      const rangeEnd = Math.min(meta.lineCount, end + OVERSCAN);

      for (let i = rangeStart; i < rangeEnd; i++) {
        if (!lineCache.current.has(i)) {
          // Find the chunk boundary
          const chunkStart = Math.floor(i / CHUNK_SIZE) * CHUNK_SIZE;
          const chunkEnd = Math.min(chunkStart + CHUNK_SIZE, meta.lineCount);
          const key = `${chunkStart}-${chunkEnd}`;
          if (!pendingFetches.current.has(key)) {
            pendingFetches.current.add(key);
            needed.push({ chunkStart, chunkEnd });
          }
          // Skip to end of this chunk
          i = chunkEnd - 1;
        }
      }

      return needed;
    },
    [meta.lineCount],
  );

  // Update rendered lines from cache
  const updateRenderedLines = useCallback(
    (start: number, end: number) => {
      const rangeStart = Math.max(0, start - OVERSCAN);
      const rangeEnd = Math.min(meta.lineCount, end + OVERSCAN);
      const lines: { idx: number; text: string }[] = [];
      for (let i = rangeStart; i < rangeEnd; i++) {
        lines.push({ idx: i, text: lineCache.current.get(i) ?? "" });
      }
      setRenderedLines(lines);
    },
    [meta.lineCount],
  );

  // Fetch chunks and update display
  const loadRange = useCallback(
    async (start: number, end: number) => {
      const chunks = fetchChunksForRange(start, end);

      if (chunks.length === 0) {
        updateRenderedLines(start, end);
        return;
      }

      // Fetch all needed chunks in parallel
      const results = await Promise.all(
        chunks.map(({ chunkStart, chunkEnd }) =>
          invoke<string[]>("read_file_chunk", {
            path: filePath,
            projectPath,
            startLine: chunkStart,
            maxLines: chunkEnd - chunkStart,
          }).then((lines) => ({ chunkStart, lines })),
        ),
      );

      // Populate cache
      for (const { chunkStart, lines } of results) {
        for (let i = 0; i < lines.length; i++) {
          lineCache.current.set(chunkStart + i, lines[i]);
        }
        pendingFetches.current.delete(`${chunkStart}-${chunkStart + lines.length}`);
      }

      updateRenderedLines(start, end);
    },
    [fetchChunksForRange, updateRenderedLines, filePath, projectPath],
  );

  // Handle scroll
  const handleScroll = useCallback(() => {
    const el = containerRef.current;
    if (!el) return;

    const scrollTop = el.scrollTop;
    const clientHeight = el.clientHeight;
    const start = Math.floor(scrollTop / LINE_HEIGHT);
    const end = Math.ceil((scrollTop + clientHeight) / LINE_HEIGHT);

    setVisibleRange({ start, end });
  }, []);

  // Load data when visible range changes
  useEffect(() => {
    loadRange(visibleRange.start, visibleRange.end);
  }, [visibleRange, loadRange]);

  // Initial load
  useEffect(() => {
    // Reset cache for new file
    lineCache.current.clear();
    pendingFetches.current.clear();
    setRenderedLines([]);
    setVisibleRange({ start: 0, end: 100 });
  }, [filePath]);

  // Line number gutter width
  const gutterWidth = useMemo(() => {
    const digits = String(meta.lineCount).length;
    return Math.max(digits * 8 + 16, 48);
  }, [meta.lineCount]);

  // File size label
  const sizeLabel = useMemo(() => {
    if (meta.sizeBytes >= 1024 * 1024) {
      return `${(meta.sizeBytes / 1024 / 1024).toFixed(1)} MB`;
    }
    return `${(meta.sizeBytes / 1024).toFixed(1)} KB`;
  }, [meta.sizeBytes]);

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        minWidth: 0,
        minHeight: 0,
      }}
    >
      {/* Info bar */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 12,
          padding: "8px 16px",
          borderBottom: "1px solid var(--border-dim)",
          background: "color-mix(in srgb, var(--bg-card) 90%, transparent)",
          fontSize: 11.5,
          color: "var(--text-secondary)",
          fontWeight: 500,
        }}
      >
        <span
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 6,
            padding: "4px 10px",
            borderRadius: 999,
            background: "color-mix(in srgb, var(--warning, #f59e0b) 12%, var(--bg-card))",
            border: "1px solid color-mix(in srgb, var(--warning, #f59e0b) 20%, var(--border-dim))",
            color: "var(--warning, #f59e0b)",
            fontWeight: 600,
          }}
        >
          Read-only
        </span>
        <span>{sizeLabel}</span>
        <span>·</span>
        <span>{meta.lineCount.toLocaleString()} lines</span>
        <span style={{ marginLeft: "auto", fontSize: 11, color: "var(--text-hint)" }}>
          Virtual scroll · large file mode
        </span>
      </div>

      {/* Virtual scroll container */}
      <div
        ref={containerRef}
        onScroll={handleScroll}
        style={{
          flex: 1,
          overflow: "auto",
          position: "relative",
          fontFamily: "JetBrains Mono, monospace",
          fontSize: 13,
          lineHeight: `${LINE_HEIGHT}px`,
          color: "var(--text-primary)",
          background: "var(--bg-card)",
        }}
      >
        {/* Spacer for total content height */}
        <div style={{ height: totalHeight, position: "relative" }}>
          {renderedLines.map(({ idx, text }) => (
            <div
              key={idx}
              style={{
                position: "absolute",
                top: idx * LINE_HEIGHT,
                left: 0,
                right: 0,
                height: LINE_HEIGHT,
                display: "flex",
                whiteSpace: "pre",
              }}
            >
              {/* Line number gutter */}
              <span
                style={{
                  width: gutterWidth,
                  flexShrink: 0,
                  textAlign: "right",
                  paddingRight: 12,
                  color: "var(--text-hint)",
                  userSelect: "none",
                  fontSize: 12,
                }}
              >
                {idx + 1}
              </span>
              {/* Line content */}
              <span
                style={{
                  flex: 1,
                  paddingLeft: 8,
                  overflow: "hidden",
                }}
              >
                {text}
              </span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
