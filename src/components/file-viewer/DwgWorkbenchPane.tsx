import { AlertCircle, LoaderCircle, Map, MapPinned, MousePointer2, ScanSearch } from "lucide-react";
import { useMemo, type ReactNode } from "react";
import type { CadReviewIssue } from "../../types";
import { DwgReviewPanel } from "./dwg/DwgReviewPanel";
import { useCadReviewRuns } from "./dwg/useCadReviewRuns";
import { useDwgIndex } from "./dwg/useDwgIndex";
import { useDwgViewerSession } from "./dwg/useDwgViewerSession";

export function DwgWorkbenchPane({
  tabId,
  active,
  filePath,
  fileName,
  projectPath,
  workspaceId,
  isDark,
  activeReviewRunId,
  activeIssueId,
  onLocateResultMessage,
  onActiveReviewRunChange,
  onActiveIssueChange,
}: {
  tabId: string;
  active: boolean;
  filePath: string;
  fileName: string;
  projectPath: string;
  workspaceId: string | null;
  isDark: boolean;
  activeReviewRunId: string | null;
  activeIssueId: string | null;
  onLocateResultMessage?: (messageId: string | null) => void;
  onActiveReviewRunChange: (runId: string | null) => void;
  onActiveIssueChange: (issueId: string | null) => void;
}) {
  const { loading, parseStatus, error: indexError, summary, docId, bytes } = useDwgIndex({
    filePath,
    fileName,
    projectPath,
  });
  const { reviewRuns, reviewDetail, reviewError } = useCadReviewRuns({
    workspaceId,
    filePath,
    activeReviewRunId,
    activeIssueId,
    onLocateResultMessage,
    onActiveReviewRunChange,
    onActiveIssueChange,
  });
  const activeIssue = useMemo<CadReviewIssue | null>(
    () => reviewDetail?.issues.find((issue) => issue.id === activeIssueId) ?? null,
    [activeIssueId, reviewDetail],
  );
  const {
    containerRef,
    loadingViewer,
    error: viewerError,
    viewMode,
    viewerNotice,
    switchToSelect,
    switchToPan,
  } = useDwgViewerSession({
    tabId,
    active,
    filePath,
    fileName,
    workspaceId,
    isDark,
    bytes,
    parseStatus,
    docId,
    parseError: indexError,
    reviewIssues: reviewDetail?.issues ?? [],
    activeIssue,
  });

  const error = viewerError ?? indexError ?? reviewError;
  const isBusy = loading || loadingViewer || parseStatus === "parsing";

  return (
    <div
      style={{
        width: "100%",
        height: "100%",
        flex: 1,
        display: "grid",
        gridTemplateColumns: "minmax(0, 1fr) minmax(320px, 360px)",
        minHeight: 0,
        minWidth: 0,
        gap: 0,
        overflow: "hidden",
      }}
    >
      <section
        style={{
          width: "100%",
          minWidth: 0,
          minHeight: 0,
          display: "flex",
          overflow: "hidden",
          position: "relative",
          background:
            "radial-gradient(circle at top left, color-mix(in srgb, var(--accent) 12%, transparent), transparent 26%), var(--bg-panel)",
        }}
      >
        <div style={{ position: "absolute", inset: 0, minHeight: 0 }}>
          <div ref={containerRef} style={{ position: "absolute", inset: 0 }} />
          {isBusy && (
            <OverlayState
              icon={<LoaderCircle size={18} style={{ animation: "spin 1.2s linear infinite" }} />}
              title={loadingViewer ? "正在加载 DWG…" : "正在解析实体索引…"}
              detail={
                loadingViewer
                  ? "准备图纸字节流并初始化 CAD Viewer。"
                  : "首次解析完成后会写入渐进式索引缓存。"
              }
            />
          )}
          {error && (
            <OverlayState
              icon={<AlertCircle size={18} />}
              title="DWG 加载失败"
              detail={error}
              tone="error"
            />
          )}
        </div>

        <div
          style={{
            position: "absolute",
            top: 16,
            left: 16,
            right: 16,
            display: "flex",
            justifyContent: "space-between",
            alignItems: "flex-start",
            gap: 12,
            pointerEvents: "none",
          }}
        >
          <div
            style={{
              maxWidth: "min(680px, calc(100% - 180px))",
              display: "grid",
              gap: 10,
              pointerEvents: "auto",
            }}
          >
            <div
              style={{
                padding: "12px 14px",
                borderRadius: 18,
                border: "1px solid color-mix(in srgb, var(--accent) 10%, var(--border-dim))",
                background: "color-mix(in srgb, var(--bg-card) 90%, transparent)",
                boxShadow: "0 14px 40px rgba(15, 23, 42, 0.12)",
                backdropFilter: "blur(12px)",
                WebkitBackdropFilter: "blur(12px)",
              }}
            >
              <div
                style={{
                  fontSize: 11,
                  fontWeight: 700,
                  letterSpacing: "0.12em",
                  color: "var(--text-hint)",
                }}
              >
                DWG 工作台
              </div>
              <div style={{ marginTop: 4, fontSize: 19, fontWeight: 700, color: "var(--text-primary)" }}>
                {fileName}
              </div>
              <div
                style={{
                  marginTop: 6,
                  fontSize: 12,
                  color: "var(--text-muted)",
                  fontFamily: "var(--font-mono)",
                  wordBreak: "break-all",
                }}
              >
                {filePath}
              </div>
            </div>

            <div style={{ display: "flex", flexWrap: "wrap", gap: 10 }}>
              <ActionChip
                active={viewMode === "select"}
                onClick={switchToSelect}
                icon={<MousePointer2 size={14} />}
              >
                选择模式
              </ActionChip>
              <ActionChip
                active={viewMode === "pan"}
                onClick={switchToPan}
                icon={<MapPinned size={14} />}
              >
                平移模式
              </ActionChip>
            </div>
          </div>

          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              flexWrap: "wrap",
              justifyContent: "flex-end",
              pointerEvents: "auto",
            }}
          >
            <StatusPill icon={<ScanSearch size={13} />} label={parseLabel(parseStatus)} />
            <StatusPill
              icon={viewMode === "select" ? <MousePointer2 size={13} /> : <Map size={13} />}
              label={viewMode === "select" ? "选择" : "平移"}
            />
            {docId && <StatusPill label="索引已关联" />}
          </div>
        </div>

        {viewerNotice && (
          <div
            style={{
              position: "absolute",
              left: 16,
              right: 16,
              bottom: 16,
              display: "flex",
              justifyContent: "flex-start",
              pointerEvents: "none",
            }}
          >
            <div
              style={{
                maxWidth: 480,
                padding: "10px 12px",
                borderRadius: 16,
                border: "1px solid var(--border-dim)",
                background: "color-mix(in srgb, var(--bg-card) 92%, transparent)",
                color: "var(--text-secondary)",
                fontSize: 12.5,
                lineHeight: 1.6,
                boxShadow: "0 12px 30px rgba(15, 23, 42, 0.1)",
                backdropFilter: "blur(10px)",
                WebkitBackdropFilter: "blur(10px)",
              }}
            >
              {viewerNotice}
            </div>
          </div>
        )}
      </section>

      <aside
        style={{
          width: "100%",
          minWidth: 0,
          minHeight: 0,
          overflow: "auto",
          padding: 16,
          display: "flex",
          flexDirection: "column",
          gap: 16,
          borderLeft: "1px solid var(--border-dim)",
          background: "color-mix(in srgb, var(--bg-card) 90%, transparent)",
        }}
      >
        <DwgReviewPanel
          summary={summary}
          reviewRuns={reviewRuns}
          reviewDetail={reviewDetail}
          activeReviewRunId={activeReviewRunId}
          activeIssueId={activeIssueId}
          onActiveReviewRunChange={onActiveReviewRunChange}
          onActiveIssueChange={onActiveIssueChange}
        />
      </aside>
    </div>
  );
}

function parseLabel(status: "idle" | "parsing" | "ready" | "error") {
  switch (status) {
    case "parsing":
      return "解析中";
    case "ready":
      return "已缓存";
    case "error":
      return "失败";
    case "idle":
    default:
      return "待命";
  }
}

function StatusPill({ icon, label }: { icon?: ReactNode; label: string }) {
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        padding: "6px 10px",
        borderRadius: 999,
        border: "1px solid var(--border-dim)",
        background: "color-mix(in srgb, var(--bg-card) 88%, transparent)",
        fontSize: 11.5,
        fontWeight: 600,
        color: "var(--text-secondary)",
      }}
    >
      {icon}
      {label}
    </span>
  );
}

function ActionChip({
  active,
  onClick,
  icon,
  children,
}: {
  active: boolean;
  onClick: () => void;
  icon: ReactNode;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 8,
        padding: "8px 12px",
        borderRadius: 999,
        border: active ? "1px solid var(--accent)" : "1px solid var(--border-dim)",
        background: active
          ? "color-mix(in srgb, var(--accent) 14%, var(--bg-card))"
          : "color-mix(in srgb, var(--bg-card) 88%, transparent)",
        color: active ? "var(--accent)" : "var(--text-secondary)",
        cursor: "pointer",
        fontSize: 12,
        fontWeight: 600,
        boxShadow: "0 10px 24px rgba(15, 23, 42, 0.08)",
        backdropFilter: "blur(10px)",
        WebkitBackdropFilter: "blur(10px)",
      }}
    >
      {icon}
      {children}
    </button>
  );
}

function OverlayState({
  icon,
  title,
  detail,
  tone = "default",
}: {
  icon: ReactNode;
  title: string;
  detail: string;
  tone?: "default" | "error";
}) {
  return (
    <div
      style={{
        position: "absolute",
        inset: 24,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        pointerEvents: "none",
      }}
    >
      <div
        style={{
          maxWidth: 420,
          padding: 18,
          borderRadius: 18,
          border: `1px solid ${tone === "error" ? "rgba(239,68,68,0.24)" : "var(--border-dim)"}`,
          background: "color-mix(in srgb, var(--bg-card) 92%, transparent)",
          color: tone === "error" ? "var(--danger)" : "var(--text-secondary)",
          textAlign: "center",
          display: "grid",
          gap: 10,
        }}
      >
        <div style={{ display: "flex", justifyContent: "center" }}>{icon}</div>
        <div style={{ fontSize: 14, fontWeight: 700 }}>{title}</div>
        <div style={{ fontSize: 12.5, lineHeight: 1.5 }}>{detail}</div>
      </div>
    </div>
  );
}
