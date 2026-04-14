import {
  Suspense,
  lazy,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import * as Popover from "@radix-ui/react-popover";
import {
  AlertCircle,
  CheckCircle2,
  Eye,
  FileCode2,
  FileText,
  ImageIcon,
  MoreHorizontal,
  PencilLine,
  X,
} from "lucide-react";
import { getFileColor } from "../utils";
import { ImagePreviewPane } from "./file-viewer/ImagePreviewPane";
import { LargeFileViewer } from "./file-viewer/LargeFileViewer";
import { MarkdownRenderer } from "./markdown/MarkdownRenderer";
import type { OpenFileTab } from "../hooks/useProjectPanels";
import type { MonacoEditorHandle } from "./file-viewer/MonacoEditorPane";
import type * as MonacoTypes from "monaco-editor";

const MonacoEditorPane = lazy(async () => {
  const module = await import("./file-viewer/MonacoEditorPane");
  return { default: module.MonacoEditorPane };
});

function isMarkdownFile(fileName: string): boolean {
  const ext = fileName.split(".").pop()?.toLowerCase();
  return ext === "md" || ext === "mdx" || ext === "markdown";
}

function isPreviewableImageFile(fileName: string): boolean {
  const ext = fileName.split(".").pop()?.toLowerCase();
  return (
    ext === "png" ||
    ext === "jpg" ||
    ext === "jpeg" ||
    ext === "gif" ||
    ext === "webp" ||
    ext === "bmp" ||
    ext === "svg"
  );
}

function getMonacoLanguage(fileName: string) {
  const normalized = fileName.toLowerCase();
  const nameMap: Record<string, string> = {
    dockerfile: "dockerfile",
    makefile: "makefile",
    ".gitignore": "shell",
    ".dockerignore": "shell",
    ".env": "shell",
    ".env.local": "shell",
    ".env.example": "shell",
    ".npmrc": "ini",
    "readme.md": "markdown",
    "readme.mdx": "markdown",
    "changelog.md": "markdown",
  };

  if (nameMap[normalized]) {
    return nameMap[normalized];
  }

  const extension = normalized.split(".").pop() ?? "";
  const extensionMap: Record<string, string> = {
    ts: "typescript",
    tsx: "typescript",
    js: "javascript",
    jsx: "javascript",
    mjs: "javascript",
    cjs: "javascript",
    json: "json",
    jsonc: "json",
    md: "markdown",
    mdx: "markdown",
    py: "python",
    rs: "rust",
    go: "go",
    java: "java",
    c: "cpp",
    h: "cpp",
    cpp: "cpp",
    cc: "cpp",
    hpp: "cpp",
    css: "css",
    scss: "scss",
    sass: "scss",
    html: "html",
    htm: "html",
    xml: "xml",
    yml: "yaml",
    yaml: "yaml",
    toml: "ini",
    sh: "shell",
    bash: "shell",
    zsh: "shell",
    fish: "shell",
    sql: "sql",
    lua: "lua",
    swift: "swift",
    kt: "kotlin",
    rb: "ruby",
    r: "r",
    proto: "protobuf",
  };

  return extensionMap[extension] ?? "plaintext";
}

type SaveStatus = "idle" | "saving" | "saved" | "error";
type ImagePreviewData = {
  dataUrl: string;
  mimeType: string;
  byteLength: number;
};

type FileMeta = {
  sizeBytes: number;
  lineCount: number;
  isText: boolean;
};

/** Threshold above which files use the virtual-scroll read-only LargeFileViewer */
const LARGE_FILE_THRESHOLD = 2 * 1024 * 1024; // 2MB

// ─── Cache types for tab pooling ────────────────────────────────────────────
type TabCache = {
  content: string;
  /** Content snapshot at last successful save — used to skip redundant writes */
  savedContent: string;
};

function FileStatusPill({
  children,
  tone = "default",
}: {
  children: ReactNode;
  tone?: "default" | "success" | "error";
}) {
  const toneStyles: Record<NonNullable<typeof tone>, CSSProperties> = {
    default: {
      color: "var(--text-secondary)",
      background: "color-mix(in srgb, var(--bg-card) 84%, transparent)",
      border: "1px solid var(--border-dim)",
    },
    success: {
      color: "var(--success)",
      background: "color-mix(in srgb, var(--success) 10%, var(--bg-card))",
      border: "1px solid color-mix(in srgb, var(--success) 18%, var(--border-dim))",
    },
    error: {
      color: "var(--danger)",
      background: "color-mix(in srgb, var(--danger) 10%, var(--bg-card))",
      border: "1px solid color-mix(in srgb, var(--danger) 20%, var(--border-dim))",
    },
  };

  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        padding: "7px 11px",
        borderRadius: 999,
        fontSize: 11.5,
        fontWeight: 600,
        whiteSpace: "nowrap",
        ...toneStyles[tone],
      }}
    >
      {children}
    </span>
  );
}

// ─── Image Preview sub-component (standalone per tab) ───────────────────────
function ImageFilePane({
  filePath,
  fileName,
  projectPath,
}: {
  filePath: string;
  fileName: string;
  projectPath: string;
}) {
  const [imagePreview, setImagePreview] = useState<ImagePreviewData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setImagePreview(null);
    setError(null);

    invoke<ImagePreviewData>("read_image_preview", { path: filePath, projectPath })
      .then((preview) => {
        if (!cancelled) {
          setImagePreview(preview);
          setLoading(false);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(String(err));
          setLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [filePath, projectPath]);

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        minWidth: 0,
        minHeight: 0,
        padding: 18,
        gap: 16,
        background:
          "radial-gradient(circle at top left, color-mix(in srgb, var(--accent) 8%, transparent), transparent 28%), var(--bg-panel)",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "flex-start",
          justifyContent: "space-between",
          gap: 16,
          padding: "18px 18px 0",
        }}
      >
        <div style={{ minWidth: 0 }}>
          <div
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: 8,
              marginBottom: 10,
              fontSize: 11,
              fontWeight: 700,
              letterSpacing: "0.12em",
              textTransform: "uppercase",
              color: "var(--text-hint)",
            }}
          >
            <ImageIcon size={13} />
            Image Preview
          </div>
          <div
            style={{
              fontSize: 24,
              fontWeight: 700,
              lineHeight: 1.05,
              letterSpacing: "-0.04em",
              color: "var(--text-primary)",
              wordBreak: "break-word",
            }}
          >
            {fileName}
          </div>
          <div
            style={{
              marginTop: 8,
              fontSize: 12,
              color: "var(--text-muted)",
              fontFamily: "var(--font-mono)",
              wordBreak: "break-all",
            }}
          >
            {filePath}
          </div>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap", justifyContent: "flex-end" }}>
          {imagePreview && (
            <FileStatusPill>{`${imagePreview.mimeType} · ${(imagePreview.byteLength / 1024).toFixed(1)} KB`}</FileStatusPill>
          )}
        </div>
      </div>

      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          minWidth: 0,
          minHeight: 0,
          overflow: "hidden",
          borderRadius: 30,
          border: "1px solid color-mix(in srgb, var(--accent) 8%, var(--border-dim))",
          background: "color-mix(in srgb, var(--bg-card) 94%, transparent)",
          boxShadow: "0 24px 70px rgba(15, 23, 42, 0.08)",
        }}
      >
        {loading && (
          <div
            style={{
              height: "100%",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              color: "var(--text-muted)",
              fontSize: 13,
            }}
          >
            Loading...
          </div>
        )}
        {error && !loading && (
          <div
            style={{
              height: "100%",
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              justifyContent: "center",
              gap: 12,
              padding: 24,
              color: "var(--text-muted)",
            }}
          >
            <AlertCircle size={28} strokeWidth={1.7} />
            <div style={{ fontSize: 13.5, maxWidth: 520, textAlign: "center" }}>{error}</div>
          </div>
        )}
        {!loading && !error && imagePreview && (
          <ImagePreviewPane
            src={imagePreview.dataUrl}
            fileName={fileName}
            mimeType={imagePreview.mimeType}
            byteLength={imagePreview.byteLength}
          />
        )}
      </div>
    </div>
  );
}

// ─── Text file header (shared by editor + markdown preview) ─────────────────
function TextFileHeader({
  fileName,
  filePath,
  language,
  saveStatus,
  isMarkdown,
  previewMode,
}: {
  fileName: string;
  filePath: string;
  language: string;
  saveStatus: SaveStatus;
  isMarkdown: boolean;
  previewMode: boolean;
}) {
  const modeLabel = isMarkdown && previewMode ? "Markdown Preview" : "Editor";
  const saveLabel =
    saveStatus === "saving"
      ? "Saving..."
      : saveStatus === "saved"
        ? "Saved"
        : saveStatus === "error"
          ? "Save failed"
          : "Live editing";

  return (
    <div
      style={{
        display: "flex",
        alignItems: "flex-start",
        justifyContent: "space-between",
        gap: 16,
        padding: "18px 18px 0",
      }}
    >
      <div style={{ minWidth: 0 }}>
        <div
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 8,
            marginBottom: 10,
            fontSize: 11,
            fontWeight: 700,
            letterSpacing: "0.12em",
            textTransform: "uppercase",
            color: "var(--text-hint)",
          }}
        >
          {isMarkdown ? <FileText size={13} /> : <FileCode2 size={13} />}
          {modeLabel}
        </div>
        <div
          style={{
            fontSize: 24,
            fontWeight: 700,
            lineHeight: 1.05,
            letterSpacing: "-0.04em",
            color: "var(--text-primary)",
            wordBreak: "break-word",
          }}
        >
          {fileName}
        </div>
        <div
          style={{
            marginTop: 8,
            fontSize: 12,
            color: "var(--text-muted)",
            fontFamily: "var(--font-mono)",
            wordBreak: "break-all",
          }}
        >
          {filePath}
        </div>
      </div>

      <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap", justifyContent: "flex-end" }}>
        <FileStatusPill>{language}</FileStatusPill>
        <FileStatusPill tone={saveStatus === "error" ? "error" : saveStatus === "saved" ? "success" : "default"}>
          {saveStatus === "saved" && <CheckCircle2 size={13} />}
          {saveLabel}
        </FileStatusPill>
      </div>
    </div>
  );
}

// ─── Main FileViewer ────────────────────────────────────────────────────────
export function FileViewer({
  tabs,
  activeFilePath,
  projectPath,
  onSelectTab,
  onCloseTab,
  onCloseOtherTabs,
  onCloseTabsToRight,
  onCloseAllTabs,
  isDark,
  onRunMakeTarget: _onRunMakeTarget,
}: {
  tabs: OpenFileTab[];
  activeFilePath: string | null;
  projectPath: string;
  onSelectTab: (path: string) => void;
  onCloseTab: (path: string) => void;
  onCloseOtherTabs: (path: string) => void;
  onCloseTabsToRight: (path: string) => void;
  onCloseAllTabs: () => void;
  isDark: boolean;
  onRunMakeTarget?: (target: string) => void;
}) {
  const [previewModes, setPreviewModes] = useState<Record<string, boolean>>({});
  const [menuOpen, setMenuOpen] = useState(false);

  // ─── Content & view state caches (survives tab switches) ────────────
  const contentCacheRef = useRef<Map<string, TabCache>>(new Map());
  const viewStateCacheRef = useRef<Map<string, MonacoTypes.editor.ICodeEditorViewState>>(new Map());
  const fileMetaCacheRef = useRef<Map<string, FileMeta>>(new Map());
  const editorHandleRef = useRef<MonacoEditorHandle | null>(null);

  // ─── Loading / error state for the active text file ─────────────────
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saveStatus, setSaveStatus] = useState<SaveStatus>("idle");
  const [activeContent, setActiveContent] = useState<string | null>(null);
  /** Set when the active file is too large for Monaco — triggers LargeFileViewer */
  const [activeFileMeta, setActiveFileMeta] = useState<FileMeta | null>(null);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savedResetRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Track which file path we last loaded to detect switches
  const prevActivePathRef = useRef<string | null>(null);

  useEffect(() => {
    setPreviewModes((prev) => {
      const next: Record<string, boolean> = {};
      for (const tab of tabs) {
        if (prev[tab.path]) {
          next[tab.path] = true;
        }
      }
      return Object.keys(next).length === Object.keys(prev).length ? prev : next;
    });

    // Clean up caches for closed tabs
    const tabPaths = new Set(tabs.map((t) => t.path));
    for (const key of contentCacheRef.current.keys()) {
      if (!tabPaths.has(key)) {
        contentCacheRef.current.delete(key);
        viewStateCacheRef.current.delete(key);
        fileMetaCacheRef.current.delete(key);
      }
    }
  }, [tabs]);

  const activeTab = useMemo(
    () => tabs.find((tab) => tab.path === activeFilePath) ?? tabs[tabs.length - 1] ?? null,
    [tabs, activeFilePath],
  );

  const activeIsImage = activeTab ? isPreviewableImageFile(activeTab.name) : false;
  const activeIsMarkdown = activeTab ? isMarkdownFile(activeTab.name) : false;
  const activePreviewMode = activeTab ? !!previewModes[activeTab.path] : false;
  const language = useMemo(
    () => (activeTab ? getMonacoLanguage(activeTab.name) : "plaintext"),
    [activeTab],
  );

  // ─── Load file content when active tab changes ─────────────────────
  useEffect(() => {
    if (!activeTab || activeIsImage) return;

    const prevPath = prevActivePathRef.current;
    prevActivePathRef.current = activeTab.path;

    // Save view state of previous tab
    if (prevPath && prevPath !== activeTab.path && editorHandleRef.current) {
      const vs = editorHandleRef.current.saveViewState();
      if (vs) {
        viewStateCacheRef.current.set(prevPath, vs);
      }
    }

    // Check if we already know this is a large file
    const cachedMeta = fileMetaCacheRef.current.get(activeTab.path);
    if (cachedMeta && cachedMeta.sizeBytes >= LARGE_FILE_THRESHOLD) {
      setActiveFileMeta(cachedMeta);
      setActiveContent(null);
      setError(null);
      setLoading(false);
      return;
    }

    // Check content cache first
    const cached = contentCacheRef.current.get(activeTab.path);
    if (cached) {
      setActiveContent(cached.content);
      setActiveFileMeta(null);
      setError(null);
      setLoading(false);
      setSaveStatus("idle");

      // If Monaco is already mounted, switch its model
      if (editorHandleRef.current && prevPath !== activeTab.path) {
        editorHandleRef.current.setValue(
          cached.content,
          activeTab.path,
          getMonacoLanguage(activeTab.name),
        );
        // Restore view state if available
        const vs = viewStateCacheRef.current.get(activeTab.path);
        if (vs) {
          editorHandleRef.current.restoreViewState(vs);
        }
      }
      return;
    }

    // Load from backend — first check file meta to decide rendering strategy
    let cancelled = false;
    setLoading(true);
    setError(null);
    setActiveContent(null);
    setActiveFileMeta(null);
    setSaveStatus("idle");

    invoke<FileMeta>("get_file_meta", { path: activeTab.path, projectPath })
      .then((meta) => {
        if (cancelled) return;
        fileMetaCacheRef.current.set(activeTab.path, meta);

        if (meta.sizeBytes >= LARGE_FILE_THRESHOLD) {
          // Large file → use virtual-scroll viewer, skip full content load
          setActiveFileMeta(meta);
          setLoading(false);
          return;
        }

        // Normal file → load full content for Monaco
        return invoke<string>("read_file_content", { path: activeTab.path, projectPath })
          .then((content) => {
            if (cancelled) return;
            contentCacheRef.current.set(activeTab.path, {
              content,
              savedContent: content,
            });
            setActiveContent(content);
            setLoading(false);

            // If Monaco is already mounted, load into it
            if (editorHandleRef.current) {
              editorHandleRef.current.setValue(
                content,
                activeTab.path,
                getMonacoLanguage(activeTab.name),
              );
            }
          });
      })
      .catch((err) => {
        if (!cancelled) {
          setError(String(err));
          setLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTab?.path, activeIsImage, projectPath]);

  // Cleanup timers on unmount
  useEffect(
    () => () => {
      if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
      if (savedResetRef.current) clearTimeout(savedResetRef.current);
    },
    [],
  );

  // ─── handleChange: NO React setState for content — ref only ────────
  const handleChange = useCallback(
    (value: string) => {
      if (!activeTab) return;

      // Update cache (ref — no re-render)
      const cache = contentCacheRef.current.get(activeTab.path);
      if (cache) {
        cache.content = value;
      } else {
        contentCacheRef.current.set(activeTab.path, { content: value, savedContent: value });
      }

      // Debounced save
      if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
      if (savedResetRef.current) clearTimeout(savedResetRef.current);

      // Skip save if content hasn't actually changed from last saved version
      const savedContent = contentCacheRef.current.get(activeTab.path)?.savedContent;
      if (savedContent === value) {
        setSaveStatus("idle");
        return;
      }

      setSaveStatus("saving");
      saveTimerRef.current = setTimeout(async () => {
        try {
          await invoke("write_file_content", {
            path: activeTab.path,
            content: value,
            projectPath,
          });
          // Update saved snapshot
          const c = contentCacheRef.current.get(activeTab.path);
          if (c) c.savedContent = value;

          setSaveStatus("saved");
          savedResetRef.current = setTimeout(() => setSaveStatus("idle"), 1800);
        } catch {
          setSaveStatus("error");
        }
      }, 900);
    },
    [activeTab, projectPath],
  );

  if (!activeTab) {
    return null;
  }

  const canCloseOtherTabs = tabs.length > 1;
  const activeTabIndex = tabs.findIndex((tab) => tab.path === activeTab.path);
  const canCloseTabsToRight = activeTabIndex !== -1 && activeTabIndex < tabs.length - 1;

  // ─── Determine what to render in the content area ─────────────────
  let contentPane: ReactNode = null;

  if (activeIsImage) {
    contentPane = (
      <ImageFilePane
        filePath={activeTab.path}
        fileName={activeTab.name}
        projectPath={projectPath}
      />
    );
  } else if (loading) {
    contentPane = (
      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          minWidth: 0,
          minHeight: 0,
          padding: 18,
          gap: 16,
          background:
            "radial-gradient(circle at top left, color-mix(in srgb, var(--accent) 8%, transparent), transparent 28%), var(--bg-panel)",
        }}
      >
        <TextFileHeader
          fileName={activeTab.name}
          filePath={activeTab.path}
          language={language}
          saveStatus={saveStatus}
          isMarkdown={activeIsMarkdown}
          previewMode={activePreviewMode}
        />
        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            minWidth: 0,
            minHeight: 0,
            overflow: "hidden",
            borderRadius: 30,
            border: "1px solid color-mix(in srgb, var(--accent) 8%, var(--border-dim))",
            background: "color-mix(in srgb, var(--bg-card) 94%, transparent)",
            boxShadow: "0 24px 70px rgba(15, 23, 42, 0.08)",
          }}
        >
          <div
            style={{
              height: "100%",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              color: "var(--text-muted)",
              fontSize: 13,
            }}
          >
            Loading...
          </div>
        </div>
      </div>
    );
  } else if (error) {
    contentPane = (
      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          minWidth: 0,
          minHeight: 0,
          padding: 18,
          gap: 16,
          background:
            "radial-gradient(circle at top left, color-mix(in srgb, var(--accent) 8%, transparent), transparent 28%), var(--bg-panel)",
        }}
      >
        <TextFileHeader
          fileName={activeTab.name}
          filePath={activeTab.path}
          language={language}
          saveStatus={saveStatus}
          isMarkdown={activeIsMarkdown}
          previewMode={activePreviewMode}
        />
        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            minWidth: 0,
            minHeight: 0,
            overflow: "hidden",
            borderRadius: 30,
            border: "1px solid color-mix(in srgb, var(--accent) 8%, var(--border-dim))",
            background: "color-mix(in srgb, var(--bg-card) 94%, transparent)",
            boxShadow: "0 24px 70px rgba(15, 23, 42, 0.08)",
          }}
        >
          <div
            style={{
              height: "100%",
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              justifyContent: "center",
              gap: 12,
              padding: 24,
              color: "var(--text-muted)",
            }}
          >
            <AlertCircle size={28} strokeWidth={1.7} />
            <div style={{ fontSize: 13.5, maxWidth: 520, textAlign: "center" }}>{error}</div>
          </div>
        </div>
      </div>
    );
  } else if (activeFileMeta) {
    // ─── Large file: virtual-scroll read-only viewer ──────────────
    contentPane = (
      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          minWidth: 0,
          minHeight: 0,
          padding: 18,
          gap: 16,
          background:
            "radial-gradient(circle at top left, color-mix(in srgb, var(--accent) 8%, transparent), transparent 28%), var(--bg-panel)",
        }}
      >
        <TextFileHeader
          fileName={activeTab.name}
          filePath={activeTab.path}
          language={language}
          saveStatus="idle"
          isMarkdown={activeIsMarkdown}
          previewMode={false}
        />
        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            minWidth: 0,
            minHeight: 0,
            overflow: "hidden",
            borderRadius: 30,
            border: "1px solid color-mix(in srgb, var(--accent) 8%, var(--border-dim))",
            background: "color-mix(in srgb, var(--bg-card) 94%, transparent)",
            boxShadow: "0 24px 70px rgba(15, 23, 42, 0.08)",
          }}
        >
          <LargeFileViewer
            filePath={activeTab.path}
            projectPath={projectPath}
            meta={activeFileMeta}
          />
        </div>
      </div>
    );
  } else if (activeContent !== null) {
    contentPane = (
      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          minWidth: 0,
          minHeight: 0,
          padding: 18,
          gap: 16,
          background:
            "radial-gradient(circle at top left, color-mix(in srgb, var(--accent) 8%, transparent), transparent 28%), var(--bg-panel)",
        }}
      >
        <TextFileHeader
          fileName={activeTab.name}
          filePath={activeTab.path}
          language={language}
          saveStatus={saveStatus}
          isMarkdown={activeIsMarkdown}
          previewMode={activePreviewMode}
        />
        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            minWidth: 0,
            minHeight: 0,
            overflow: "hidden",
            borderRadius: 30,
            border: "1px solid color-mix(in srgb, var(--accent) 8%, var(--border-dim))",
            background: "color-mix(in srgb, var(--bg-card) 94%, transparent)",
            boxShadow: "0 24px 70px rgba(15, 23, 42, 0.08)",
          }}
        >
          {activeIsMarkdown && activePreviewMode ? (
            <div className="md-preview-shell" style={{ height: "100%", overflow: "auto" }}>
              <div className="md-preview-card">
                <div className="md-preview-header">
                  <div>
                    <div className="md-preview-eyebrow">Rendered Markdown</div>
                    <div className="md-preview-title">{activeTab.name}</div>
                    <div className="md-preview-subtitle">{projectPath}</div>
                  </div>
                  <div className="md-preview-meta">react-markdown + remark-gfm + rehype-raw</div>
                </div>
                <div className="md-preview-body">
                  <MarkdownRenderer content={activeContent} variant="document" />
                </div>
              </div>
            </div>
          ) : (
            <Suspense fallback={<div className="monaco-loading">Loading editor...</div>}>
              <MonacoEditorPane
                ref={editorHandleRef}
                initialValue={activeContent}
                filePath={activeTab.path}
                language={language}
                isDark={isDark}
                onChange={handleChange}
              />
            </Suspense>
          )}
        </div>
      </div>
    );
  }

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        minWidth: 0,
        minHeight: 0,
        background:
          "linear-gradient(180deg, color-mix(in srgb, var(--bg-sidebar) 64%, transparent), var(--bg-panel))",
      }}
    >
      {/* ─── Tab strip ─────────────────────────────────────────────── */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          minWidth: 0,
          padding: "10px 12px",
          borderBottom: "1px solid var(--border-dim)",
          background: "color-mix(in srgb, var(--bg-card) 74%, transparent)",
          backdropFilter: "blur(14px)",
          WebkitBackdropFilter: "blur(14px)",
        }}
      >
        <div
          className="file-viewer-tab-strip"
          style={{
            flex: 1,
            minWidth: 0,
            display: "flex",
            alignItems: "center",
            gap: 6,
            overflowX: "auto",
            overflowY: "hidden",
            paddingBottom: 2,
          }}
        >
          {tabs.map((tab) => {
            const isActive = tab.path === activeTab.path;
            const fileColor = getFileColor(tab.name);

            return (
              <button
                key={tab.path}
                type="button"
                onClick={() => onSelectTab(tab.path)}
                title={tab.path}
                style={{
                  minWidth: 0,
                  maxWidth: 260,
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  padding: "10px 12px",
                  borderRadius: 16,
                  border: isActive
                    ? "1px solid color-mix(in srgb, var(--accent) 24%, var(--border-dim))"
                    : "1px solid transparent",
                  background: isActive
                    ? "linear-gradient(135deg, color-mix(in srgb, var(--accent) 9%, var(--bg-card)), color-mix(in srgb, var(--bg-card) 88%, transparent))"
                    : "transparent",
                  color: isActive ? "var(--text-primary)" : "var(--text-secondary)",
                  cursor: "pointer",
                  flexShrink: 0,
                  boxShadow: isActive ? "0 10px 24px rgba(15, 23, 42, 0.05)" : "none",
                }}
              >
                <span
                  style={{
                    width: 8,
                    height: 8,
                    borderRadius: 999,
                    background: fileColor,
                    flexShrink: 0,
                    boxShadow: `0 0 0 4px color-mix(in srgb, ${fileColor} 16%, transparent)`,
                  }}
                />
                <span
                  style={{
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                    fontSize: 12.5,
                    fontWeight: isActive ? 600 : 500,
                  }}
                >
                  {tab.name}
                </span>
                <span
                  onClick={(event) => {
                    event.stopPropagation();
                    onCloseTab(tab.path);
                  }}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    width: 20,
                    height: 20,
                    borderRadius: 999,
                    color: "var(--text-hint)",
                    flexShrink: 0,
                  }}
                  role="button"
                  aria-label={`Close ${tab.name}`}
                >
                  <X size={12} />
                </span>
              </button>
            );
          })}
        </div>

        {activeIsMarkdown && (
          <button
            type="button"
            onClick={() =>
              setPreviewModes((prev) => ({
                ...prev,
                [activeTab.path]: !prev[activeTab.path],
              }))
            }
            title={activePreviewMode ? "Switch to editor" : "Switch to preview"}
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: 8,
              padding: "8px 12px",
              borderRadius: 999,
              border: "1px solid color-mix(in srgb, var(--accent) 16%, var(--border-dim))",
              background: activePreviewMode
                ? "color-mix(in srgb, var(--accent) 10%, var(--bg-card))"
                : "color-mix(in srgb, var(--bg-card) 90%, transparent)",
              color: activePreviewMode ? "var(--accent)" : "var(--text-secondary)",
              cursor: "pointer",
              fontSize: 12,
              fontWeight: 600,
              flexShrink: 0,
            }}
          >
            {activePreviewMode ? <PencilLine size={14} /> : <Eye size={14} />}
            {activePreviewMode ? "Edit Markdown" : "Preview Markdown"}
          </button>
        )}

        <Popover.Root open={menuOpen} onOpenChange={setMenuOpen}>
          <Popover.Trigger asChild>
            <button
              type="button"
              title="Tab actions"
              aria-label="Tab actions"
              style={{
                width: 36,
                height: 36,
                borderRadius: 12,
                border: "1px solid var(--border-dim)",
                background: "color-mix(in srgb, var(--bg-card) 88%, transparent)",
                color: "var(--text-secondary)",
                cursor: "pointer",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                flexShrink: 0,
              }}
            >
              <MoreHorizontal size={15} />
            </button>
          </Popover.Trigger>
          <Popover.Portal>
            <Popover.Content
              sideOffset={6}
              align="end"
              onOpenAutoFocus={(event) => event.preventDefault()}
              className="file-viewer-tab-menu"
            >
              <button
                type="button"
                disabled={!canCloseOtherTabs}
                onClick={() => {
                  onCloseOtherTabs(activeTab.path);
                  setMenuOpen(false);
                }}
                className="file-viewer-tab-menu-item"
              >
                Close Other Tabs
              </button>
              <button
                type="button"
                disabled={!canCloseTabsToRight}
                onClick={() => {
                  onCloseTabsToRight(activeTab.path);
                  setMenuOpen(false);
                }}
                className="file-viewer-tab-menu-item"
              >
                Close Tabs to the Right
              </button>
              <button
                type="button"
                disabled={tabs.length === 0}
                onClick={() => {
                  onCloseAllTabs();
                  setMenuOpen(false);
                }}
                className="file-viewer-tab-menu-item"
              >
                Close All Tabs
              </button>
            </Popover.Content>
          </Popover.Portal>
        </Popover.Root>
      </div>

      {/* ─── Content area: only active tab is rendered ───────────── */}
      <div
        style={{
          flex: 1,
          position: "relative",
          minWidth: 0,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
        }}
      >
        {contentPane}
      </div>
    </div>
  );
}
