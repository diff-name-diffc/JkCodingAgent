import {
  Suspense,
  lazy,
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
import { MarkdownRenderer } from "./markdown/MarkdownRenderer";
import type { OpenFileTab } from "../hooks/useProjectPanels";

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

function FilePreviewPane({
  filePath,
  fileName,
  projectPath,
  isDark,
  previewMode,
}: {
  filePath: string;
  fileName: string;
  projectPath: string;
  isDark: boolean;
  previewMode: boolean;
}) {
  const [content, setContent] = useState<string | null>(null);
  const [imagePreview, setImagePreview] = useState<ImagePreviewData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saveStatus, setSaveStatus] = useState<SaveStatus>("idle");
  const isMarkdown = isMarkdownFile(fileName);
  const isPreviewableImage = isPreviewableImageFile(fileName);
  const language = useMemo(() => getMonacoLanguage(fileName), [fileName]);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savedResetRef = useRef<ReturnType<typeof setTimeout> | null>(null);


  useEffect(() => {
    let cancelled = false;

    setLoading(true);
    setContent(null);
    setImagePreview(null);
    setError(null);
    setSaveStatus("idle");

    const loadTask = isPreviewableImage
      ? invoke<ImagePreviewData>("read_image_preview", { path: filePath, projectPath }).then((preview) => {
          if (!cancelled) {
            setImagePreview(preview);
            setLoading(false);
          }
        })
      : invoke<string>("read_file_content", { path: filePath, projectPath }).then((nextContent) => {
          if (!cancelled) {
            setContent(nextContent);
            setLoading(false);
          }
        });

    loadTask.catch((err) => {
      if (!cancelled) {
        setError(String(err));
        setLoading(false);
      }
    });

    return () => {
      cancelled = true;
    };
  }, [filePath, isPreviewableImage, projectPath]);

  useEffect(
    () => () => {
      if (saveTimerRef.current) {
        clearTimeout(saveTimerRef.current);
      }
      if (savedResetRef.current) {
        clearTimeout(savedResetRef.current);
      }
    },
    [],
  );

  function handleChange(value: string) {
    setContent(value);

    if (saveTimerRef.current) {
      clearTimeout(saveTimerRef.current);
    }
    if (savedResetRef.current) {
      clearTimeout(savedResetRef.current);
    }

    setSaveStatus("saving");
    saveTimerRef.current = setTimeout(async () => {
      try {
        await invoke("write_file_content", { path: filePath, content: value, projectPath });
        setSaveStatus("saved");
        savedResetRef.current = setTimeout(() => setSaveStatus("idle"), 1800);
      } catch {
        setSaveStatus("error");
      }
    }, 900);
  }

  const modeLabel = isPreviewableImage ? "Image Preview" : isMarkdown && previewMode ? "Markdown Preview" : "Editor";
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
            {isPreviewableImage ? <ImageIcon size={13} /> : isMarkdown ? <FileText size={13} /> : <FileCode2 size={13} />}
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
          {!isPreviewableImage && <FileStatusPill>{language}</FileStatusPill>}
          {!isPreviewableImage && (
            <FileStatusPill tone={saveStatus === "error" ? "error" : saveStatus === "saved" ? "success" : "default"}>
              {saveStatus === "saved" && <CheckCircle2 size={13} />}
              {saveLabel}
            </FileStatusPill>
          )}
          {isPreviewableImage && imagePreview && (
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

        {!loading &&
          !error &&
          (isPreviewableImage && imagePreview ? (
            <ImagePreviewPane
              src={imagePreview.dataUrl}
              fileName={fileName}
              mimeType={imagePreview.mimeType}
              byteLength={imagePreview.byteLength}
            />
          ) : content !== null ? (
            isMarkdown && previewMode ? (
              <div className="md-preview-shell" style={{ height: "100%", overflow: "auto" }}>
                <div className="md-preview-card">
                  <div className="md-preview-header">
                    <div>
                      <div className="md-preview-eyebrow">Rendered Markdown</div>
                      <div className="md-preview-title">{fileName}</div>
                      <div className="md-preview-subtitle">{projectPath}</div>
                    </div>
                    <div className="md-preview-meta">react-markdown + remark-gfm + rehype-raw</div>
                  </div>
                  <div className="md-preview-body">
                    <MarkdownRenderer content={content} variant="document" />
                  </div>
                </div>
              </div>
            ) : (
              <Suspense fallback={<div className="monaco-loading">Loading editor...</div>}>
                <MonacoEditorPane
                  filePath={filePath}
                  value={content}
                  language={language}
                  isDark={isDark}
                  onChange={handleChange}
                />
              </Suspense>
            )
          ) : null)}
      </div>
    </div>
  );
}

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
  }, [tabs]);

  const activeTab = useMemo(
    () => tabs.find((tab) => tab.path === activeFilePath) ?? tabs[tabs.length - 1] ?? null,
    [tabs, activeFilePath],
  );

  if (!activeTab) {
    return null;
  }

  const activePreviewMode = !!previewModes[activeTab.path];
  const activeIsMarkdown = isMarkdownFile(activeTab.name);
  const canCloseOtherTabs = tabs.length > 1;
  const activeTabIndex = tabs.findIndex((tab) => tab.path === activeTab.path);
  const canCloseTabsToRight = activeTabIndex !== -1 && activeTabIndex < tabs.length - 1;

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

      <div
        style={{
          flex: 1,
          position: "relative",
          minWidth: 0,
          minHeight: 0,
        }}
      >
        {tabs.map((tab) => {
          const isActive = tab.path === activeTab.path;

          return (
            <div
              key={tab.path}
              style={{
                position: "absolute",
                inset: 0,
                display: "flex",
                flexDirection: "column",
                visibility: isActive ? "visible" : "hidden",
                pointerEvents: isActive ? "auto" : "none",
              }}
            >
              <FilePreviewPane
                filePath={tab.path}
                fileName={tab.name}
                projectPath={projectPath}
                isDark={isDark}
                previewMode={!!previewModes[tab.path]}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}
