import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertCircle, CheckCircle2, Eye, ImageIcon, PencilLine } from "lucide-react";
import { LargeFileViewer } from "./LargeFileViewer";
import { MonacoEditorPane } from "./MonacoEditorPane";
import { ImagePreviewPane } from "./ImagePreviewPane";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";
import { FileGlyph, resolveFilePresentation, type FilePresentation } from "../../file-icons";
import type { OpenFileTab } from "../../hooks/useProjectPanels";

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

const LARGE_FILE_THRESHOLD = 2 * 1024 * 1024;

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
        padding: "5px 10px",
        borderRadius: 999,
        fontSize: 11,
        fontWeight: 600,
        whiteSpace: "nowrap",
        ...toneStyles[tone],
      }}
    >
      {children}
    </span>
  );
}

function PaneShell({
  children,
  padding = 18,
  gap = 16,
}: {
  children: ReactNode;
  padding?: number;
  gap?: number;
}) {
  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        minWidth: 0,
        minHeight: 0,
        padding,
        gap,
        background:
          "radial-gradient(circle at top left, color-mix(in srgb, var(--accent) 8%, transparent), transparent 28%), var(--bg-panel)",
      }}
    >
      {children}
    </div>
  );
}

function PaneCard({ children }: { children: ReactNode }) {
  return (
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
      {children}
    </div>
  );
}

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
    <PaneShell>
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
            图片预览
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

      <PaneCard>
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
            加载中...
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
      </PaneCard>
    </PaneShell>
  );
}

function TextFileHeader({
  presentation,
  fileName,
  filePath,
  language,
  saveStatus,
  isMarkdown,
  previewMode,
  onTogglePreview,
}: {
  presentation: FilePresentation;
  fileName: string;
  filePath: string;
  language: string;
  saveStatus: SaveStatus;
  isMarkdown: boolean;
  previewMode: boolean;
  onTogglePreview?: () => void;
}) {
  const saveLabel =
    saveStatus === "saving"
      ? "保存中..."
      : saveStatus === "saved"
        ? "已保存"
        : saveStatus === "error"
          ? "保存失败"
          : "实时编辑";
  const normalizedPath = filePath.replace(/\\/g, "/");
  const lastSlashIndex = normalizedPath.lastIndexOf("/");
  const directoryPath = lastSlashIndex >= 0 ? normalizedPath.slice(0, lastSlashIndex + 1) : "";
  const displayFileName = lastSlashIndex >= 0 ? normalizedPath.slice(lastSlashIndex + 1) : fileName;

  return (
    <div
      style={{
        display: "flex",
        flexWrap: "wrap",
        alignItems: "center",
        justifyContent: "space-between",
        gap: 12,
        padding: 0,
        minHeight: 28,
      }}
    >
      <div style={{ minWidth: 0, flex: 1 }}>
        <div
          style={{
            display: "flex",
            alignItems: "baseline",
            minWidth: 0,
            overflow: "hidden",
            fontSize: 12.5,
            lineHeight: 1.35,
            color: "var(--text-muted)",
            fontFamily: "var(--font-mono)",
          }}
        >
          <span style={{ display: "inline-flex", alignItems: "center", marginRight: 8, flexShrink: 0 }}>
            <FileGlyph presentation={presentation} size={22} />
          </span>
          {directoryPath ? (
            <span
              style={{
                minWidth: 0,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {directoryPath}
            </span>
          ) : null}
          <strong
            style={{
              color: "var(--text-primary)",
              fontWeight: 700,
              whiteSpace: "nowrap",
              flexShrink: 0,
            }}
          >
            {displayFileName}
          </strong>
        </div>
      </div>

      <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap", justifyContent: "flex-end" }}>
        {isMarkdown && onTogglePreview && (
          <button
            type="button"
            onClick={onTogglePreview}
            title={previewMode ? "切换到编辑" : "切换到预览"}
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: 6,
              padding: "6px 10px",
              borderRadius: 999,
              border: "1px solid color-mix(in srgb, var(--accent) 16%, var(--border-dim))",
              background: previewMode
                ? "color-mix(in srgb, var(--accent) 10%, var(--bg-card))"
                : "color-mix(in srgb, var(--bg-card) 90%, transparent)",
              color: previewMode ? "var(--accent)" : "var(--text-secondary)",
              cursor: "pointer",
              fontSize: 11.5,
              fontWeight: 600,
              flexShrink: 0,
            }}
          >
            {previewMode ? <PencilLine size={14} /> : <Eye size={14} />}
            {previewMode ? "编辑" : "预览"}
          </button>
        )}
        <FileStatusPill>{language}</FileStatusPill>
        <FileStatusPill tone={saveStatus === "error" ? "error" : saveStatus === "saved" ? "success" : "default"}>
          {saveStatus === "saved" && <CheckCircle2 size={13} />}
          {saveLabel}
        </FileStatusPill>
      </div>
    </div>
  );
}

export function FileTabPane({
  active,
  tab,
  projectPath,
  isDark,
}: {
  active: boolean;
  tab: OpenFileTab;
  projectPath: string;
  isDark: boolean;
}) {
  const [previewMode, setPreviewMode] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saveStatus, setSaveStatus] = useState<SaveStatus>("idle");
  const [content, setContent] = useState<string | null>(null);
  const [fileMeta, setFileMeta] = useState<FileMeta | null>(null);
  const [largeDirty, setLargeDirty] = useState(false);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savedResetRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savedContentRef = useRef("");
  const queuedSaveContentRef = useRef<string | null>(null);
  const saveInFlightRef = useRef(false);

  const presentation = useMemo(
    () => resolveFilePresentation({ name: tab.name, path: tab.path }),
    [tab.name, tab.path],
  );
  const isImage = presentation.isPreviewableImage;
  const isMarkdown = presentation.isMarkdown;
  const language = presentation.monacoLanguage;

  useEffect(() => {
    setPreviewMode(false);
  }, [tab.path]);

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

  const flushQueuedSave = useCallback(async () => {
    if (saveInFlightRef.current) {
      return;
    }

    saveInFlightRef.current = true;
    try {
      while (queuedSaveContentRef.current !== null) {
        const contentToSave = queuedSaveContentRef.current;
        queuedSaveContentRef.current = null;

        await invoke("write_file_content", {
          path: tab.path,
          content: contentToSave,
          projectPath,
        });

        savedContentRef.current = contentToSave;
      }

      setSaveStatus("saved");
      if (savedResetRef.current) {
        clearTimeout(savedResetRef.current);
      }
      savedResetRef.current = setTimeout(() => setSaveStatus("idle"), 1800);
    } catch {
      setSaveStatus("error");
    } finally {
      saveInFlightRef.current = false;
      if (queuedSaveContentRef.current !== null) {
        void flushQueuedSave();
      }
    }
  }, [projectPath, tab.path]);

  useEffect(() => {
    if (isImage) {
      return;
    }

    let cancelled = false;
    setLoading(true);
    setError(null);
    setSaveStatus("idle");
    setContent(null);
    setFileMeta(null);
    setLargeDirty(false);
    queuedSaveContentRef.current = null;
    saveInFlightRef.current = false;

    invoke<FileMeta>("get_file_meta", { path: tab.path, projectPath })
      .then((meta) => {
        if (cancelled) {
          return;
        }

        if (meta.sizeBytes >= LARGE_FILE_THRESHOLD) {
          setFileMeta(meta);
          setLoading(false);
          return;
        }

        invoke<string>("read_file_content", { path: tab.path, projectPath })
          .then((nextContent) => {
            if (cancelled) {
              return;
            }

            savedContentRef.current = nextContent;
            setContent(nextContent);
            setFileMeta(meta);
            setLoading(false);
          })
          .catch((err) => {
            if (!cancelled) {
              setError(String(err));
              setLoading(false);
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
  }, [isImage, projectPath, tab.path]);

  const handleChange = useCallback(
    (value: string) => {
      setContent(value);

      if (saveTimerRef.current) {
        clearTimeout(saveTimerRef.current);
      }
      if (savedResetRef.current) {
        clearTimeout(savedResetRef.current);
      }

      if (savedContentRef.current === value) {
        setSaveStatus("idle");
        return;
      }

      setSaveStatus("saving");
      saveTimerRef.current = setTimeout(async () => {
        try {
          queuedSaveContentRef.current = value;
          await flushQueuedSave();
        } catch {
          setSaveStatus("error");
        }
      }, 900);
    },
    [flushQueuedSave],
  );

  if (isImage) {
    return <ImageFilePane filePath={tab.path} fileName={tab.name} projectPath={projectPath} />;
  }

  return (
    <PaneShell padding={14} gap={10}>
              <TextFileHeader
                presentation={presentation}
                fileName={tab.name}
                filePath={tab.path}
                language={language}
        saveStatus={fileMeta && fileMeta.sizeBytes >= LARGE_FILE_THRESHOLD ? (largeDirty ? "saving" : "idle") : saveStatus}
        isMarkdown={isMarkdown}
        previewMode={previewMode}
        onTogglePreview={isMarkdown ? () => setPreviewMode((prev) => !prev) : undefined}
      />

      <PaneCard>
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
            加载中...
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

        {!loading && !error && fileMeta && fileMeta.sizeBytes >= LARGE_FILE_THRESHOLD && (
          <LargeFileViewer
            active={active}
            sessionId={tab.id}
            filePath={tab.path}
            projectPath={projectPath}
            meta={fileMeta}
            onDirtyChange={setLargeDirty}
          />
        )}

        {!loading && !error && content !== null && fileMeta && fileMeta.sizeBytes < LARGE_FILE_THRESHOLD && (
          isMarkdown && previewMode ? (
            <div className="md-preview-shell" style={{ height: "100%", overflow: "auto" }}>
              <div className="md-preview-card">
                <div className="md-preview-header">
                  <div>
                    <div className="md-preview-eyebrow">Markdown 预览</div>
                    <div className="md-preview-title">{tab.name}</div>
                    <div className="md-preview-subtitle">{projectPath}</div>
                  </div>
                  <div className="md-preview-meta">基于 react-markdown 渲染</div>
                </div>
                <div className="md-preview-body">
                  <MarkdownRenderer content={content} variant="document" />
                </div>
              </div>
            </div>
          ) : (
            <MonacoEditorPane
              active={active}
              initialValue={content}
              filePath={tab.path}
              language={language}
              isDark={isDark}
              onChange={handleChange}
            />
          )
        )}
      </PaneCard>
    </PaneShell>
  );
}
