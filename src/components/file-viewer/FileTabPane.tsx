import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertCircle } from "lucide-react";
import { LargeFileViewer } from "./LargeFileViewer";
import { MonacoEditorPane } from "./MonacoEditorPane";
import { ImageFilePane } from "./ImageFilePane";
import { FilePaneHeader, type FileSaveStatus } from "./FilePaneHeader";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";
import { resolveFilePresentation } from "../../file-icons";
import type { EditorTab } from "../../hooks/useProjectPanels";

type OpenFileTab = Extract<EditorTab, { kind: "file" }>;

type SaveStatus = "idle" | "saving" | "saved" | "error";

type FileMeta = {
  sizeBytes: number;
  lineCount: number;
  isText: boolean;
};

const LARGE_FILE_THRESHOLD = 2 * 1024 * 1024;

export function FileTabPane({
  active,
  tab,
  projectPath,
  onDirtyChange,
}: {
  active: boolean;
  tab: OpenFileTab;
  projectPath: string;
  /** 未保存状态上报（UI-16）：标签条据此渲染脏标记圆点。 */
  onDirtyChange?: (dirty: boolean) => void;
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

  const isLargeFile = fileMeta !== null && fileMeta.sizeBytes >= LARGE_FILE_THRESHOLD;
  /** 脏定义：小文件在防抖保存中/保存失败，大文件有未落盘编辑；saved 短暂回显不算。 */
  const dirty = isLargeFile ? largeDirty : saveStatus === "saving" || saveStatus === "error";

  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);

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

  const headerSaveStatus: FileSaveStatus = isLargeFile ? (largeDirty ? "unsaved" : "idle") : saveStatus;

  return (
    <div className="ai-file-pane">
      <FilePaneHeader
        projectPath={projectPath}
        filePath={tab.path}
        saveStatus={headerSaveStatus}
        isMarkdown={isMarkdown}
        previewMode={previewMode}
        onTogglePreview={isMarkdown ? () => setPreviewMode((prev) => !prev) : undefined}
      />

      <div className="ai-file-pane-body">
        {loading && <div className="ai-file-pane-state">加载中...</div>}

        {error && !loading && (
          <div className="ai-file-pane-state is-error">
            <AlertCircle size={28} strokeWidth={1.7} />
            <div>{error}</div>
          </div>
        )}

        {!loading && !error && isLargeFile && (
          <LargeFileViewer
            active={active}
            sessionId={tab.id}
            filePath={tab.path}
            projectPath={projectPath}
            meta={fileMeta}
            onDirtyChange={setLargeDirty}
          />
        )}

        {!loading && !error && content !== null && !isLargeFile && (
          isMarkdown && previewMode ? (
            <div className="md-preview-shell">
              <div className="md-preview-card">
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
              language={presentation.monacoLanguage}
              onChange={handleChange}
            />
          )
        )}
      </div>
    </div>
  );
}
