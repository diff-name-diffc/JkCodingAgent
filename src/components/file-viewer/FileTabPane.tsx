import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
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
  return <span className={`ai-file-status-pill is-${tone}`}>{children}</span>;
}

function PaneShell({
  children,
  compact = false,
}: {
  children: ReactNode;
  compact?: boolean;
}) {
  return (
    <div className={compact ? "ai-file-pane-shell is-compact" : "ai-file-pane-shell"}>
      {children}
    </div>
  );
}

function PaneCard({ children }: { children: ReactNode }) {
  return <div className="ai-file-pane-card">{children}</div>;
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
      <div className="ai-image-pane-header">
        <div className="ai-image-pane-title-block">
          <div className="ai-image-pane-eyebrow">
            <ImageIcon size={13} />
            图片预览
          </div>
          <div className="ai-image-pane-title">
            {fileName}
          </div>
          <div className="ai-image-pane-path">
            {filePath}
          </div>
        </div>
        <div className="ai-image-pane-meta">
          {imagePreview && (
            <FileStatusPill>{`${imagePreview.mimeType} · ${(imagePreview.byteLength / 1024).toFixed(1)} KB`}</FileStatusPill>
          )}
        </div>
      </div>

      <PaneCard>
        {loading && (
          <div className="ai-file-pane-state">
            加载中...
          </div>
        )}
        {error && !loading && (
          <div className="ai-file-pane-state is-error">
            <AlertCircle size={28} strokeWidth={1.7} />
            <div>{error}</div>
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
    <div className="ai-file-pane-header">
      <div className="ai-file-path-stack">
        <div className="ai-file-path-line">
          <span className="ai-file-path-icon">
            <FileGlyph presentation={presentation} size={22} />
          </span>
          {directoryPath ? (
            <span className="ai-file-path-dir">
              {directoryPath}
            </span>
          ) : null}
          <strong className="ai-file-path-name">
            {displayFileName}
          </strong>
        </div>
      </div>

      <div className="ai-file-pane-actions">
        {isMarkdown && onTogglePreview && (
          <button
            type="button"
            onClick={onTogglePreview}
            title={previewMode ? "切换到编辑" : "切换到预览"}
            className={previewMode ? "ai-file-preview-toggle is-active" : "ai-file-preview-toggle"}
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
}: {
  active: boolean;
  tab: OpenFileTab;
  projectPath: string;
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
  const contentRef = useRef<string | null>(null);
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
    contentRef.current = null;
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
            contentRef.current = nextContent;
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
      contentRef.current = value;
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
    <PaneShell compact>
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
          <div className="ai-file-pane-state">
            加载中...
          </div>
        )}

        {error && !loading && (
          <div className="ai-file-pane-state is-error">
            <AlertCircle size={28} strokeWidth={1.7} />
            <div>{error}</div>
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
            <div className="md-preview-shell">
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
              onChange={handleChange}
            />
          )
        )}
      </PaneCard>
    </PaneShell>
  );
}
