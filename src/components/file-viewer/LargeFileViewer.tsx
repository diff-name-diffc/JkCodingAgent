import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

const LINE_HEIGHT = 22;
const OVERSCAN = 40;
const CHUNK_SIZE = 200;

type FileMeta = {
  sizeBytes: number;
  lineCount: number;
  isText: boolean;
};

type RopeMeta = {
  lineCount: number;
  charCount: number;
  byteLen: number;
};

type RopeEditResult = {
  lineCount: number;
  affectedStartLine: number;
  affectedEndLine: number;
};

// ─── Cursor helpers ────────────────────────────────────────────────
function setCaretPosition(el: HTMLElement, offset: number) {
  const textNode = el.firstChild;
  if (!textNode) {
    el.focus();
    return;
  }
  const range = document.createRange();
  const sel = window.getSelection();
  const maxOffset = textNode.textContent?.length ?? 0;
  range.setStart(textNode, Math.min(offset, maxOffset));
  range.collapse(true);
  sel?.removeAllRanges();
  sel?.addRange(range);
}

function getCaretOffset(): number {
  const sel = window.getSelection();
  return sel?.focusOffset ?? 0;
}

/**
 * Virtual-scrolling text editor for large files.
 * Backed by ropey on the Rust side for O(log N) editing.
 */
export function LargeFileViewer({
  active,
  sessionId,
  filePath,
  projectPath,
  meta,
  onDirtyChange,
}: {
  active: boolean;
  sessionId: string;
  filePath: string;
  projectPath: string;
  meta: FileMeta;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const contentAreaRef = useRef<HTMLDivElement>(null);
  const [visibleRange, setVisibleRange] = useState({ start: 0, end: 100 });
  const lineCache = useRef<Map<number, string>>(new Map());
  const syncedLineCache = useRef<Map<number, string>>(new Map());
  const [renderedLines, setRenderedLines] = useState<{ idx: number; text: string }[]>([]);
  const pendingFetches = useRef<Set<string>>(new Set());
  const [ropeReady, setRopeReady] = useState(false);
  const [totalLines, setTotalLines] = useState(meta.lineCount);
  const [dirty, setDirty] = useState(false);
  const editingLineRef = useRef<number | null>(null);
  const editTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Pending focus target after re-render from structural edits (Enter / Paste)
  const pendingFocusRef = useRef<{ line: number; col: number } | null>(null);
  const pendingLocalEditRef = useRef<{ line: number; content: string } | null>(null);

  const totalHeight = totalLines * LINE_HEIGHT;

  // ─── Open file in rope on mount ──────────────────────────────────
  useEffect(() => {
    let cancelled = false;
    lineCache.current.clear();
    syncedLineCache.current.clear();
    pendingFetches.current.clear();
    setRenderedLines([]);
    setRopeReady(false);
    setDirty(false);
    setVisibleRange({ start: 0, end: 100 });

    invoke<RopeMeta>("rope_open", { sessionId, path: filePath, projectPath })
      .then((ropeMeta) => {
        if (cancelled) return;
        setTotalLines(ropeMeta.lineCount);
        setRopeReady(true);
      })
      .catch((err) => console.error("Failed to open rope:", err));

    return () => {
      cancelled = true;
      invoke("rope_close", { sessionId }).catch(() => {});
    };
  }, [filePath, projectPath, sessionId]);

  // ─── Compute needed chunks ───────────────────────────────────────
  const fetchChunksForRange = useCallback(
    (start: number, end: number) => {
      const needed: { chunkStart: number; chunkEnd: number }[] = [];
      const rangeStart = Math.max(0, start - OVERSCAN);
      const rangeEnd = Math.min(totalLines, end + OVERSCAN);

      for (let i = rangeStart; i < rangeEnd; i++) {
        if (!lineCache.current.has(i)) {
          const chunkStart = Math.floor(i / CHUNK_SIZE) * CHUNK_SIZE;
          const chunkEnd = Math.min(chunkStart + CHUNK_SIZE, totalLines);
          const key = `${chunkStart}-${chunkEnd}`;
          if (!pendingFetches.current.has(key)) {
            pendingFetches.current.add(key);
            needed.push({ chunkStart, chunkEnd });
          }
          i = chunkEnd - 1;
        }
      }
      return needed;
    },
    [totalLines],
  );

  // ─── Build renderedLines from cache ──────────────────────────────
  const updateRenderedLines = useCallback(
    (start: number, end: number) => {
      const rangeStart = Math.max(0, start - OVERSCAN);
      const rangeEnd = Math.min(totalLines, end + OVERSCAN);
      const lines: { idx: number; text: string }[] = [];
      for (let i = rangeStart; i < rangeEnd; i++) {
        lines.push({ idx: i, text: lineCache.current.get(i) ?? "" });
      }
      setRenderedLines(lines);
    },
    [totalLines],
  );

  // ─── Load lines from rope ────────────────────────────────────────
  const loadRange = useCallback(
    async (start: number, end: number) => {
      if (!ropeReady) return;

      const chunks = fetchChunksForRange(start, end);
      if (chunks.length === 0) {
        updateRenderedLines(start, end);
        return;
      }

      const results = await Promise.all(
        chunks.map(({ chunkStart, chunkEnd }) =>
          invoke<string[]>("rope_read_lines", {
            sessionId,
            startLine: chunkStart,
            maxLines: chunkEnd - chunkStart,
          }).then((lines) => ({ chunkStart, lines })),
        ),
      );

      for (const { chunkStart, lines } of results) {
        for (let i = 0; i < lines.length; i++) {
          const lineNumber = chunkStart + i;
          if (editingLineRef.current !== lineNumber) {
            lineCache.current.set(lineNumber, lines[i]);
            syncedLineCache.current.set(lineNumber, lines[i]);
          }
        }
        const chunkEnd = chunkStart + lines.length;
        pendingFetches.current.delete(`${chunkStart}-${chunkEnd}`);
      }

      updateRenderedLines(start, end);
    },
    [ropeReady, fetchChunksForRange, sessionId, updateRenderedLines],
  );

  // ─── Scroll handler ──────────────────────────────────────────────
  const handleScroll = useCallback(() => {
    const el = containerRef.current;
    if (!el) return;
    const scrollTop = el.scrollTop;
    const clientHeight = el.clientHeight;
    const start = Math.floor(scrollTop / LINE_HEIGHT);
    const end = Math.ceil((scrollTop + clientHeight) / LINE_HEIGHT);
    setVisibleRange({ start, end });
  }, []);

  useEffect(() => {
    loadRange(visibleRange.start, visibleRange.end);
  }, [visibleRange, loadRange]);

  useEffect(() => {
    if (!active) {
      return;
    }

    requestAnimationFrame(() => {
      handleScroll();
    });
  }, [active, handleScroll]);

  // ─── After render: apply pending focus ───────────────────────────
  useEffect(() => {
    if (!pendingFocusRef.current) return;
    const { line, col } = pendingFocusRef.current;

    // The target line may not be rendered yet — wait one rAF
    requestAnimationFrame(() => {
      const el = contentAreaRef.current?.querySelector(
        `[data-line="${line}"]`,
      ) as HTMLElement | null;
      if (el) {
        editingLineRef.current = line;
        el.focus();
        setCaretPosition(el, col);
      }
      pendingFocusRef.current = null;
    });
  }, [renderedLines]);

  // ─── Find a line's contentEditable element ───────────────────────
  const getLineElement = useCallback((lineIdx: number): HTMLElement | null => {
    return contentAreaRef.current?.querySelector(
      `[data-line="${lineIdx}"]`,
    ) as HTMLElement | null;
  }, []);

  // ─── Invalidate cache from a line onward ─────────────────────────
  const invalidateCacheFrom = useCallback((fromLine: number) => {
    for (const key of lineCache.current.keys()) {
      if (key >= fromLine) {
        lineCache.current.delete(key);
      }
    }
    for (const key of syncedLineCache.current.keys()) {
      if (key >= fromLine) {
        syncedLineCache.current.delete(key);
      }
    }
    pendingFetches.current.clear();
  }, []);

  // ─── Mark dirty ──────────────────────────────────────────────────
  const markDirty = useCallback(() => {
    setDirty(true);
    onDirtyChange?.(true);
  }, [onDirtyChange]);

  // ─── Flush pending debounced edit synchronously to rope ──────────
  const flushPendingEdit = useCallback(
    async (lineIdx: number, content: string) => {
      if (editTimerRef.current) {
        clearTimeout(editTimerRef.current);
        editTimerRef.current = null;
      }
      pendingLocalEditRef.current = null;
      const syncedContent = syncedLineCache.current.get(lineIdx) ?? "";
      if (syncedContent === content) {
        return;
      }
      await invoke<RopeEditResult>("rope_replace_line", {
        sessionId,
        line: lineIdx,
        newContent: content,
      });
      syncedLineCache.current.set(lineIdx, content);
    },
    [sessionId],
  );

  // ─── Commit a single-line edit (debounced) ───────────────────────
  const commitLineEdit = useCallback(
    (lineIdx: number, content: string) => {
      const oldContent = syncedLineCache.current.get(lineIdx) ?? "";
      if (oldContent === content) {
        pendingLocalEditRef.current = null;
        return;
      }

      pendingLocalEditRef.current = null;
      lineCache.current.set(lineIdx, content);
      markDirty();

      invoke<RopeEditResult>("rope_replace_line", {
        sessionId,
        line: lineIdx,
        newContent: content,
      })
        .then((result) => {
          syncedLineCache.current.set(lineIdx, content);
          if (result.lineCount !== totalLines) {
            setTotalLines(result.lineCount);
          }
        })
        .catch((err) => {
          console.error("rope_replace_line failed:", err);
          lineCache.current.set(lineIdx, oldContent);
          syncedLineCache.current.set(lineIdx, oldContent);
        });
    },
    [markDirty, sessionId, totalLines],
  );

  // ─── onInput: debounce single-line text changes ──────────────────
  const handleInput = useCallback(
    (lineIdx: number, el: HTMLElement) => {
      const content = el.textContent ?? "";
      lineCache.current.set(lineIdx, content);
      pendingLocalEditRef.current = { line: lineIdx, content };

      if (editTimerRef.current) clearTimeout(editTimerRef.current);
      editTimerRef.current = setTimeout(() => {
        commitLineEdit(lineIdx, content);
      }, 150);

      // Mark dirty immediately
      markDirty();
    },
    [commitLineEdit, markDirty],
  );

  // ─── Structural edit: insert text at (line, col) in rope ─────────
  // Used by Enter and Paste. This changes line count.
  const insertTextAtCursor = useCallback(
    async (lineIdx: number, col: number, text: string) => {
      const currentEl = getLineElement(lineIdx);
      const currentContent = currentEl?.textContent ?? lineCache.current.get(lineIdx) ?? "";

      // 1. Flush: make sure rope has current line content
      await flushPendingEdit(lineIdx, currentContent);
      editingLineRef.current = null;

      // 2. Insert text at (line, col) via rope_edit
      const result = await invoke<RopeEditResult>("rope_edit", {
        sessionId,
        line: lineIdx,
        col,
        deleteCount: 0,
        insertText: text,
      });

      markDirty();

      // 3. Invalidate cache from edit point onward (line indices shifted)
      invalidateCacheFrom(lineIdx);

      // 4. Update total line count
      setTotalLines(result.lineCount);

      // 5. Calculate where cursor should go after the insert
      const newlineCount = (text.match(/\n/g) || []).length;
      const afterLastNewline = text.lastIndexOf("\n");
      const focusLine = lineIdx + newlineCount;
      const focusCol =
        afterLastNewline >= 0
          ? text.length - afterLastNewline - 1
          : col + text.length;

      pendingFocusRef.current = { line: focusLine, col: focusCol };

      // 6. Force re-fetch visible lines (cache was cleared)
      // The totalLines state update + loadRange will trigger re-render
    },
    [flushPendingEdit, getLineElement, invalidateCacheFrom, markDirty, sessionId],
  );

  // ─── Keyboard handler ───────────────────────────────────────────
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLSpanElement>, lineIdx: number) => {
      if (e.key === "ArrowUp") {
        e.preventDefault();
        const caretCol = getCaretOffset();
        const currentEl = e.currentTarget;
        commitLineEdit(lineIdx, currentEl.textContent ?? "");
        editingLineRef.current = null;

        const prevLine = getLineElement(lineIdx - 1);
        if (prevLine) {
          editingLineRef.current = lineIdx - 1;
          prevLine.focus();
          setCaretPosition(prevLine, caretCol);
        }
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        const caretCol = getCaretOffset();
        const currentEl = e.currentTarget;
        commitLineEdit(lineIdx, currentEl.textContent ?? "");
        editingLineRef.current = null;

        const nextLine = getLineElement(lineIdx + 1);
        if (nextLine) {
          editingLineRef.current = lineIdx + 1;
          nextLine.focus();
          setCaretPosition(nextLine, caretCol);
        }
      } else if (e.key === "Enter") {
        e.preventDefault();
        const col = getCaretOffset();
        insertTextAtCursor(lineIdx, col, "\n");
      } else if (e.key === "Backspace" && getCaretOffset() === 0) {
        // At the beginning of a line — merge with previous line
        e.preventDefault();
        if (lineIdx === 0) return;

        const currentEl = e.currentTarget;
        const currentContent = currentEl.textContent ?? "";
        const prevContent = lineCache.current.get(lineIdx - 1) ?? "";

        // Merge: set previous line to prevContent + currentContent,
        // then delete the current line's leading newline via rope_edit
        (async () => {
          // Flush current line first
          await flushPendingEdit(lineIdx, currentContent);
          editingLineRef.current = null;

          // Delete the newline at end of previous line (merges the two lines)
          const result = await invoke<RopeEditResult>("rope_edit", {
            sessionId,
            line: lineIdx - 1,
            col: prevContent.length,
            deleteCount: 1, // delete the \n
            insertText: "",
          });

          markDirty();
          invalidateCacheFrom(lineIdx - 1);
          setTotalLines(result.lineCount);
          pendingFocusRef.current = { line: lineIdx - 1, col: prevContent.length };
        })();
      } else if (e.key === "Delete") {
        const el = e.currentTarget;
        const content = el.textContent ?? "";
        const col = getCaretOffset();
        if (col === content.length) {
          // At the end of a line — merge with next line (delete the trailing \n)
          e.preventDefault();

          (async () => {
            await flushPendingEdit(lineIdx, content);
            editingLineRef.current = null;

            const result = await invoke<RopeEditResult>("rope_edit", {
              sessionId,
              line: lineIdx,
              col: content.length,
              deleteCount: 1, // delete the \n
              insertText: "",
            });

            markDirty();
            invalidateCacheFrom(lineIdx);
            setTotalLines(result.lineCount);
            pendingFocusRef.current = { line: lineIdx, col: content.length };
          })();
        }
      } else if (e.key === "Tab") {
        e.preventDefault();
        // Insert 2 spaces at cursor
        const col = getCaretOffset();
        const el = e.currentTarget;
        const text = el.textContent ?? "";
        const newText = text.slice(0, col) + "  " + text.slice(col);
        el.textContent = newText;
        setCaretPosition(el, col + 2);
        handleInput(lineIdx, el);
      }
    },
    [commitLineEdit, getLineElement, insertTextAtCursor, flushPendingEdit,
     handleInput, invalidateCacheFrom, markDirty, sessionId],
  );

  // ─── Paste handler ──────────────────────────────────────────────
  const handlePaste = useCallback(
    (e: React.ClipboardEvent<HTMLSpanElement>, lineIdx: number) => {
      e.preventDefault();
      const pastedText = e.clipboardData.getData("text/plain");
      if (!pastedText) return;

      const col = getCaretOffset();

      if (!pastedText.includes("\n")) {
        // Single-line paste: insert inline without structural change
        const el = e.currentTarget;
        const text = el.textContent ?? "";
        const newText = text.slice(0, col) + pastedText + text.slice(col);
        el.textContent = newText;
        setCaretPosition(el, col + pastedText.length);
        handleInput(lineIdx, el);
      } else {
        // Multi-line paste: structural change via rope_edit
        insertTextAtCursor(lineIdx, col, pastedText);
      }
    },
    [handleInput, insertTextAtCursor],
  );

  // ─── Focus / blur handlers ──────────────────────────────────────
  const handleFocus = useCallback((lineIdx: number) => {
    editingLineRef.current = lineIdx;
  }, []);

  const handleBlur = useCallback(
    (lineIdx: number, el: HTMLElement) => {
      if (editTimerRef.current) {
        clearTimeout(editTimerRef.current);
        editTimerRef.current = null;
      }
      pendingLocalEditRef.current = null;
      commitLineEdit(lineIdx, el.textContent ?? "");
      editingLineRef.current = null;
    },
    [commitLineEdit],
  );

  // ─── Undo / Redo handler ────────────────────────────────────────
  const handleUndoRedo = useCallback(
    async (isRedo: boolean) => {
      let focusTarget: { line: number; col: number } | null = null;

      // Flush any pending edit first
      if (editTimerRef.current) {
        clearTimeout(editTimerRef.current);
        editTimerRef.current = null;
      }
      if (editingLineRef.current !== null) {
        const focusLine = editingLineRef.current;
        const focusCol = getCaretOffset();
        const el = getLineElement(editingLineRef.current);
        if (el) {
          await flushPendingEdit(editingLineRef.current, el.textContent ?? "");
        }
        focusTarget = { line: focusLine, col: focusCol };
        editingLineRef.current = null;
      }

      try {
        const cmd = isRedo ? "rope_redo" : "rope_undo";
        const result = await invoke<RopeMeta>(cmd, { sessionId });

        // Invalidate entire cache — rope state changed
        lineCache.current.clear();
        syncedLineCache.current.clear();
        pendingFetches.current.clear();
        setTotalLines(result.lineCount);
        setDirty(true);
        onDirtyChange?.(true);

        if (focusTarget) {
          pendingFocusRef.current = {
            line: Math.min(focusTarget.line, Math.max(result.lineCount - 1, 0)),
            col: focusTarget.col,
          };
        }

        // Force re-fetch visible lines
        loadRange(visibleRange.start, visibleRange.end);
      } catch {
        // Nothing to undo/redo — silently ignore
      }
    },
    [flushPendingEdit, getLineElement, loadRange, onDirtyChange, sessionId, visibleRange],
  );

  // ─── Save handler (Cmd+S) + Undo/Redo (Cmd+Z / Cmd+Shift+Z) ───
  useEffect(() => {
    if (!active) {
      return;
    }

    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "s") {
        e.preventDefault();
        if (!dirty) return;

        // Flush any pending edit
        if (editTimerRef.current) {
          clearTimeout(editTimerRef.current);
          editTimerRef.current = null;
        }
        if (editingLineRef.current !== null) {
          const el = getLineElement(editingLineRef.current);
          if (el) {
            commitLineEdit(editingLineRef.current, el.textContent ?? "");
          }
        }

        // Small delay to let the last rope_replace_line finish
        setTimeout(() => {
          invoke("rope_save", { sessionId, projectPath })
            .then(() => {
              setDirty(false);
              onDirtyChange?.(false);
            })
            .catch((err) => console.error("rope_save failed:", err));
        }, 200);
      } else if ((e.metaKey || e.ctrlKey) && e.key === "z") {
        e.preventDefault();
        handleUndoRedo(e.shiftKey); // shiftKey → redo
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [active, dirty, projectPath, sessionId, onDirtyChange, getLineElement, commitLineEdit, handleUndoRedo]);

  // Cleanup
  useEffect(() => {
    return () => {
      if (editTimerRef.current) clearTimeout(editTimerRef.current);
    };
  }, []);

  // ─── Gutter width ──────────────────────────────────────────────
  const gutterWidth = useMemo(() => {
    const digits = String(totalLines).length;
    return Math.max(digits * 8 + 16, 48);
  }, [totalLines]);

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
          gap: 10,
          padding: "6px 12px",
          borderBottom: "1px solid var(--border-dim)",
          background: "color-mix(in srgb, var(--bg-card) 90%, transparent)",
          fontSize: 11,
          color: "var(--text-secondary)",
          fontWeight: 500,
          flexShrink: 0,
        }}
      >
        <span
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 6,
            padding: "3px 8px",
            borderRadius: 999,
            background: dirty
              ? "color-mix(in srgb, var(--accent) 12%, var(--bg-card))"
              : "color-mix(in srgb, var(--success, #22c55e) 12%, var(--bg-card))",
            border: dirty
              ? "1px solid color-mix(in srgb, var(--accent) 20%, var(--border-dim))"
              : "1px solid color-mix(in srgb, var(--success, #22c55e) 20%, var(--border-dim))",
            color: dirty ? "var(--accent)" : "var(--success, #22c55e)",
            fontWeight: 600,
          }}
        >
          {dirty ? "Modified" : "Saved"}
        </span>
        <span>{sizeLabel}</span>
        <span>·</span>
        <span>{totalLines.toLocaleString()} lines</span>
        {dirty && <span style={{ marginLeft: "auto", fontSize: 10.5, color: "var(--text-hint)" }}>⌘S to save</span>}
      </div>

      {/* Virtual scroll container */}
      <div
        ref={containerRef}
        onScroll={handleScroll}
        tabIndex={-1}
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
        <div ref={contentAreaRef} style={{ height: totalHeight, position: "relative" }}>
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
                alignItems: "stretch",
              }}
            >
              {/* Line number gutter */}
              <span
                style={{
                  width: gutterWidth,
                  flexShrink: 0,
                  textAlign: "right",
                  paddingRight: 12,
                  color: editingLineRef.current === idx
                    ? "var(--accent)"
                    : "var(--text-hint)",
                  userSelect: "none",
                  fontSize: 12,
                  lineHeight: `${LINE_HEIGHT}px`,
                }}
              >
                {idx + 1}
              </span>
              {/* Editable line content */}
              <span
                data-line={idx}
                contentEditable
                suppressContentEditableWarning
                spellCheck={false}
                ref={(el) => {
                  // Directly set textContent via ref callback to avoid
                  // React vs contentEditable DOM conflicts (Bug 1 fix).
                  // Only update if the line is NOT currently being edited
                  // to prevent caret jumps during typing.
                  if (el && editingLineRef.current !== idx) {
                    if (el.textContent !== text) {
                      el.textContent = text;
                    }
                  }
                }}
                onFocus={() => handleFocus(idx)}
                onBlur={(e) => handleBlur(idx, e.currentTarget)}
                onInput={(e) => handleInput(idx, e.currentTarget)}
                onKeyDown={(e) => handleKeyDown(e, idx)}
                onPaste={(e) => handlePaste(e, idx)}
                style={{
                  flex: 1,
                  minWidth: 0,
                  paddingLeft: 8,
                  outline: "none",
                  whiteSpace: "pre",
                  overflow: "hidden",
                  height: LINE_HEIGHT,
                  lineHeight: `${LINE_HEIGHT}px`,
                  display: "block",
                  caretColor: "var(--accent)",
                  borderLeft: editingLineRef.current === idx
                    ? "2px solid var(--accent)"
                    : "2px solid transparent",
                  background: editingLineRef.current === idx
                    ? "color-mix(in srgb, var(--accent) 4%, transparent)"
                    : "transparent",
                }}
              />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
