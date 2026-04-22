import { invoke } from "@tauri-apps/api/core";
import { AlertCircle, LoaderCircle, Map, MapPinned, MousePointer2, ScanSearch } from "lucide-react";
import { AcGeBox2d } from "@mlightcad/data-model";
import {
  AcApContext,
  AcApDocument,
  AcApPanCmd,
  AcApSelectCmd,
  AcEdOpenMode,
  AcTrView2d,
} from "@mlightcad/cad-simple-viewer";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MutableRefObject,
  type ReactNode,
} from "react";
import type {
  CadBBox,
  CadPoint,
  CadReviewRun,
  CadReviewRunDetail,
  DwgParseCacheRecord,
  DwgParseSummary,
} from "../../types";
import { openCadViewerDwgDocument } from "../../lib/cadViewerDwg";
import { buildDwgCacheFingerprint } from "../../lib/dwgCache";

type FileMeta = {
  sizeBytes: number;
  lineCount: number;
  isText: boolean;
  modifiedAt: number;
};

type ViewerBridge = {
  context: AcApContext;
  document: AcApDocument;
  view: AcTrView2d;
};

const DWG_PARSER_VERSION = "dwg-worker-v1";

export function DwgWorkbenchPane({
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
  const containerRef = useRef<HTMLDivElement | null>(null);
  const markerRef = useRef<HTMLDivElement | null>(null);
  const markerFrameRef = useRef<number | null>(null);
  const viewerRef = useRef<ViewerBridge | null>(null);
  const [loading, setLoading] = useState(true);
  const [parseStatus, setParseStatus] = useState<"idle" | "parsing" | "ready" | "error">("idle");
  const [error, setError] = useState<string | null>(null);
  const [summary, setSummary] = useState<DwgParseSummary | null>(null);
  const [reviewRuns, setReviewRuns] = useState<CadReviewRun[]>([]);
  const [reviewDetail, setReviewDetail] = useState<CadReviewRunDetail | null>(null);
  const [viewerNotice, setViewerNotice] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState<"select" | "pan">("select");
  const worker = useMemo(
    () =>
      new Worker(new URL("../../workers/dwgParseWorker.ts", import.meta.url), { type: "module" }),
    [],
  );

  const loadReviewRuns = useCallback(async () => {
    if (!workspaceId) {
      setReviewRuns([]);
      onLocateResultMessage?.(null);
      return;
    }

    const runs = await invoke<CadReviewRun[]>("dispatcher_list_cad_review_runs", {
      workspaceId,
      filePath,
    });
    setReviewRuns(runs);
    if (runs.length === 0) {
      setReviewDetail(null);
      onActiveReviewRunChange(null);
      onActiveIssueChange(null);
      onLocateResultMessage?.(null);
      return;
    }

    const resolvedRunId =
      activeReviewRunId && runs.some((run) => run.id === activeReviewRunId)
        ? activeReviewRunId
        : runs[0].id;
    if (resolvedRunId !== activeReviewRunId) {
      onActiveReviewRunChange(resolvedRunId);
    }
  }, [
    activeReviewRunId,
    filePath,
    onActiveIssueChange,
    onActiveReviewRunChange,
    onLocateResultMessage,
    workspaceId,
  ]);

  useEffect(() => {
    let cancelled = false;

    async function boot() {
      setLoading(true);
      setError(null);
      setParseStatus("idle");

      try {
        const [meta, rawBytes] = await Promise.all([
          invoke<FileMeta>("get_file_meta", { path: filePath, projectPath }),
          invoke<number[]>("read_binary_file", { path: filePath, projectPath }),
        ]);
        if (cancelled) return;

        const bytes = Uint8Array.from(rawBytes);
        const viewerContainer = containerRef.current;
        if (!viewerContainer) {
          throw new Error("CAD 容器尚未就绪");
        }

        const view = new AcTrView2d({
          container: viewerContainer,
          background: isDark ? 0x101825 : 0xf8fafc,
          calculateSizeCallback: () => getViewerContainerSize(viewerContainer),
        });
        const document = new AcApDocument();
        const context = new AcApContext(view, document);
        await openCadViewerDwgDocument({
          document,
          fileName,
          content: bytes.slice().buffer,
          mode: AcEdOpenMode.Review,
        });
        const modelSpaceBtrId = resolveModelSpaceBtrId(document);
        view.modelSpaceBtrId = modelSpaceBtrId;
        view.activeLayoutBtrId = modelSpaceBtrId;
        view.clear();
        await document.database.regen();
        viewerRef.current = { context, document, view };
        if (document.database.extents.isEmpty()) {
          view.zoomToFitDrawing(1500);
        } else {
          view.zoomTo(new AcGeBox2d(document.database.extmin, document.database.extmax));
        }
        new AcApSelectCmd().execute(context).catch(() => undefined);
        setViewMode("select");

        const cached = await invoke<DwgParseCacheRecord | null>("dispatcher_get_dwg_parse_cache", {
          projectPath,
          filePath,
          fileSize: meta.sizeBytes,
          fileMtime: meta.modifiedAt,
          parserVersion: DWG_PARSER_VERSION,
        });
        if (cancelled) return;

        if (cached) {
          const nextFingerprint = buildDwgCacheFingerprint({
            projectPath,
            filePath,
            fileSize: meta.sizeBytes,
            fileMtime: meta.modifiedAt,
            parserVersion: DWG_PARSER_VERSION,
          });
          const cachedFingerprint = buildDwgCacheFingerprint({
            projectPath: cached.projectPath,
            filePath: cached.filePath,
            fileSize: cached.fileSize,
            fileMtime: cached.fileMtime,
            parserVersion: cached.parserVersion,
          });
          if (cachedFingerprint === nextFingerprint) {
            setSummary(cached.summary);
            setParseStatus("ready");
          } else {
            setParseStatus("parsing");
            const workerBytes = bytes.slice();
            worker.postMessage(
              {
                kind: "parse",
                filePath,
                fileName,
                parserVersion: DWG_PARSER_VERSION,
                bytes: workerBytes,
              },
              [workerBytes.buffer],
            );
          }
        } else {
          setParseStatus("parsing");
          const workerBytes = bytes.slice();
          worker.postMessage(
            {
              kind: "parse",
              filePath,
              fileName,
              parserVersion: DWG_PARSER_VERSION,
              bytes: workerBytes,
            },
            [workerBytes.buffer],
          );
        }

        await loadReviewRuns();
      } catch (nextError) {
        if (!cancelled) {
          setError(nextError instanceof Error ? nextError.message : String(nextError));
          setParseStatus("error");
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    }

    void boot();

    return () => {
      cancelled = true;
      if (markerFrameRef.current !== null) {
        cancelAnimationFrame(markerFrameRef.current);
        markerFrameRef.current = null;
      }
      markerRef.current?.remove();
      markerRef.current = null;
      const bridge = viewerRef.current;
      viewerRef.current = null;
      if (bridge) {
        bridge.view.stopAnimationLoop();
        bridge.view.clear();
      }
    };
  }, [fileName, filePath, isDark, loadReviewRuns, projectPath, worker]);

  useEffect(() => {
    const handleMessage = async (event: MessageEvent) => {
      const payload = event.data as
        | {
            kind: "parsed";
            filePath: string;
            parserVersion: string;
            summary: DwgParseSummary;
            entities: DwgParseCacheRecord["entities"];
          }
        | { kind: "error"; filePath: string; error: string };

      if (payload.filePath !== filePath) {
        return;
      }

      if (payload.kind === "error") {
        setParseStatus("error");
        setError(payload.error);
        return;
      }

      try {
        const meta = await invoke<FileMeta>("get_file_meta", { path: filePath, projectPath });
        const saved = await invoke<DwgParseCacheRecord>("dispatcher_save_dwg_parse_cache", {
          payload: {
            projectPath,
            filePath,
            fileSize: meta.sizeBytes,
            fileMtime: meta.modifiedAt,
            parserVersion: payload.parserVersion,
            summary: payload.summary,
            entities: payload.entities,
          },
        });
        setSummary(saved.summary);
        setParseStatus("ready");
      } catch (nextError) {
        setParseStatus("error");
        setError(nextError instanceof Error ? nextError.message : String(nextError));
      }
    };

    worker.addEventListener("message", handleMessage);
    return () => {
      worker.removeEventListener("message", handleMessage);
    };
  }, [filePath, projectPath, worker]);

  useEffect(
    () => () => {
      worker.terminate();
    },
    [worker],
  );

  useEffect(() => {
    if (!workspaceId || !activeReviewRunId) {
      setReviewDetail(null);
      onLocateResultMessage?.(null);
      return;
    }

    let cancelled = false;
    invoke<CadReviewRunDetail>("dispatcher_get_cad_review_run_detail", {
      workspaceId,
      runId: activeReviewRunId,
    })
      .then((detail) => {
        if (cancelled) return;
        setReviewDetail(detail);
        if (!activeIssueId || !detail.issues.some((issue) => issue.id === activeIssueId)) {
          onActiveIssueChange(detail.issues[0]?.id ?? null);
        }
      })
      .catch((nextError) => {
        if (!cancelled) {
          setError(nextError instanceof Error ? nextError.message : String(nextError));
        }
      });

    return () => {
      cancelled = true;
    };
  }, [activeIssueId, activeReviewRunId, onActiveIssueChange, onLocateResultMessage, workspaceId]);

  useEffect(() => {
    const bridge = viewerRef.current;
    const issue = reviewDetail?.issues.find((value) => value.id === activeIssueId) ?? null;
    if (!bridge || !issue) {
      setViewerNotice(null);
      bridge?.view.selectionSet.clear();
      return;
    }

    const ids = issue.entityRefs;
    const target = issue.anchorPoint ?? bboxCenter(issue.bbox ?? null);
    setViewerNotice(
      ids.length === 0 && !target
        ? "当前问题没有可定位的图元引用、锚点或包围盒，只能保留文字说明。"
        : null,
    );
    bridge.view.selectionSet.clear();
    if (ids.length > 0) {
      try {
        bridge.view.selectionSet.add(ids);
        bridge.view.highlight(ids);
      } catch (error) {
        setViewerNotice(error instanceof Error ? error.message : "问题图元高亮失败");
      }
    }

    if (target) {
      new AcApSelectCmd().execute(bridge.context).catch(() => undefined);
      bridge.view.flyTo(target, 4);
      mountMarkerLoop(bridge.view, target, markerRef, markerFrameRef);
    }

    return () => {
      if (ids.length > 0) {
        bridge.view.unhighlight(ids);
      }
    };
  }, [activeIssueId, reviewDetail]);

  useEffect(() => {
    const issue = reviewDetail?.issues.find((value) => value.id === activeIssueId) ?? null;
    if (!issue) {
      return;
    }
    onLocateResultMessage?.(reviewDetail?.run.resultMessageId ?? null);
  }, [activeIssueId, onLocateResultMessage, reviewDetail]);

  const handleSwitchToSelect = useCallback(() => {
    const bridge = viewerRef.current;
    if (!bridge) return;
    new AcApSelectCmd().execute(bridge.context).catch(() => undefined);
    setViewMode("select");
  }, []);

  const handleSwitchToPan = useCallback(() => {
    const bridge = viewerRef.current;
    if (!bridge) return;
    new AcApPanCmd().execute(bridge.context).catch(() => undefined);
    setViewMode("pan");
  }, []);

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
          {(loading || parseStatus === "parsing") && (
            <OverlayState
              icon={<LoaderCircle size={18} style={{ animation: "spin 1.2s linear infinite" }} />}
              title={loading ? "正在加载 DWG…" : "正在解析实体索引…"}
              detail={loading ? "准备图纸字节流与 CAD Viewer" : "首次解析完成后会写入会话缓存"}
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
                onClick={handleSwitchToSelect}
                icon={<MousePointer2 size={14} />}
              >
                选择模式
              </ActionChip>
              <ActionChip
                active={viewMode === "pan"}
                onClick={handleSwitchToPan}
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
        <PanelCard title="解析摘要">
          {summary ? (
            <div style={{ display: "grid", gap: 10 }}>
              <MetaRow label="实体总数" value={String(summary.totalEntities)} />
              <MetaRow label="未知实体" value={String(summary.unknownEntityCount)} />
              <MetaRow label="图层数" value={String(summary.layers.length)} />
              {summary.bounds && (
                <MetaRow
                  label="范围"
                  value={`${summary.bounds.minX.toFixed(1)}, ${summary.bounds.minY.toFixed(1)} → ${summary.bounds.maxX.toFixed(1)}, ${summary.bounds.maxY.toFixed(1)}`}
                />
              )}
              <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                {summary.layers.slice(0, 10).map((layer) => (
                  <span key={layer.name} style={tokenStyle}>
                    {layer.name} · {layer.entityCount}
                  </span>
                ))}
              </div>
            </div>
          ) : (
            <EmptyHint>还没有可展示的 DWG 摘要。</EmptyHint>
          )}
        </PanelCard>

        <PanelCard title="问题清单">
          {reviewRuns.length === 0 ? (
            <EmptyHint>当前会话还没有这个 DWG 的审查结果。</EmptyHint>
          ) : (
            <div style={{ display: "grid", gap: 12 }}>
              <div style={{ display: "grid", gap: 8 }}>
                {reviewRuns.map((run) => {
                  const active = run.id === activeReviewRunId;
                  return (
                    <button
                      key={run.id}
                      type="button"
                      onClick={() => onActiveReviewRunChange(run.id)}
                      style={{
                        textAlign: "left",
                        padding: 10,
                        borderRadius: 14,
                        border: active ? "1px solid var(--accent)" : "1px solid var(--border-dim)",
                        background: active ? "var(--accent-subtle)" : "transparent",
                        cursor: "pointer",
                      }}
                    >
                      <div style={{ fontSize: 13, fontWeight: 700, color: "var(--text-primary)" }}>
                        {run.summary}
                      </div>
                      <div style={{ fontSize: 11.5, color: "var(--text-muted)", marginTop: 4 }}>
                        {run.issueCount} 条问题 · {new Date(run.createdAt).toLocaleString()}
                      </div>
                    </button>
                  );
                })}
              </div>

              {reviewDetail && (
                <div style={{ display: "grid", gap: 8 }}>
                  {reviewDetail.issues.map((issue) => {
                    const active = issue.id === activeIssueId;
                    return (
                      <button
                        key={issue.id}
                        type="button"
                        onClick={() => onActiveIssueChange(issue.id)}
                        style={{
                          textAlign: "left",
                          padding: 12,
                          borderRadius: 14,
                          border: active
                            ? "1px solid var(--accent)"
                            : "1px solid var(--border-dim)",
                          background: active
                            ? "color-mix(in srgb, var(--accent) 10%, var(--bg-card))"
                            : "color-mix(in srgb, var(--bg-card) 90%, transparent)",
                          cursor: "pointer",
                        }}
                      >
                        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                          <span style={severityPill(issue.severity)}>{issue.severity}</span>
                          <span
                            style={{ fontSize: 13, fontWeight: 700, color: "var(--text-primary)" }}
                          >
                            {issue.title}
                          </span>
                        </div>
                        <div
                          style={{
                            marginTop: 6,
                            fontSize: 12,
                            color: "var(--text-secondary)",
                            lineHeight: 1.5,
                          }}
                        >
                          {issue.description}
                        </div>
                        {(issue.anchorPoint || issue.bbox) && (
                          <div
                            style={{
                              marginTop: 8,
                              fontSize: 11.5,
                              color: "var(--text-muted)",
                              fontFamily: "var(--font-mono)",
                            }}
                          >
                            {issue.anchorPoint
                              ? `定位: ${issue.anchorPoint.x.toFixed(2)}, ${issue.anchorPoint.y.toFixed(2)}`
                              : `范围: ${issue.bbox?.minX.toFixed(2)}, ${issue.bbox?.minY.toFixed(2)} → ${issue.bbox?.maxX.toFixed(2)}, ${issue.bbox?.maxY.toFixed(2)}`}
                          </div>
                        )}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          )}
        </PanelCard>
      </aside>
    </div>
  );
}

function getViewerContainerSize(container: HTMLDivElement) {
  const rect = container.getBoundingClientRect();
  return {
    width: Math.max(1, Math.floor(rect.width)),
    height: Math.max(1, Math.floor(rect.height)),
  };
}

function resolveModelSpaceBtrId(document: AcApDocument) {
  return document.database.tables.blockTable.modelSpace.objectId || document.database.currentSpaceId;
}

function mountMarkerLoop(
  view: AcTrView2d,
  target: CadPoint,
  markerRef: MutableRefObject<HTMLDivElement | null>,
  markerFrameRef: MutableRefObject<number | null>,
) {
  markerRef.current?.remove();
  const marker = document.createElement("div");
  marker.style.position = "absolute";
  marker.style.width = "18px";
  marker.style.height = "18px";
  marker.style.borderRadius = "999px";
  marker.style.border = "3px solid #ef4444";
  marker.style.boxShadow = "0 0 0 6px rgba(239,68,68,0.12)";
  marker.style.pointerEvents = "none";
  marker.style.zIndex = "40";
  markerRef.current = marker;
  view.container.appendChild(marker);

  const tick = () => {
    const screenPoint = view.worldToScreen(target);
    const containerPoint = view.canvasToContainer(screenPoint);
    marker.style.transform = `translate(${containerPoint.x - 9}px, ${containerPoint.y - 9}px)`;
    markerFrameRef.current = requestAnimationFrame(tick);
  };
  tick();
}

function bboxCenter(bbox: CadBBox | null): CadPoint | null {
  if (!bbox) return null;
  return {
    x: (bbox.minX + bbox.maxX) / 2,
    y: (bbox.minY + bbox.maxY) / 2,
  };
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

function StatusPill({ icon, label }: { icon: ReactNode; label: string }) {
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

function PanelCard({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section
      style={{
        borderRadius: 20,
        border: "1px solid var(--border-dim)",
        background: "color-mix(in srgb, var(--bg-card) 92%, transparent)",
        padding: 14,
        display: "grid",
        gap: 12,
      }}
    >
      <div
        style={{
          fontSize: 12,
          fontWeight: 700,
          letterSpacing: "0.08em",
          color: "var(--text-hint)",
        }}
      >
        {title}
      </div>
      {children}
    </section>
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

function MetaRow({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ display: "flex", justifyContent: "space-between", gap: 12, fontSize: 12.5 }}>
      <span style={{ color: "var(--text-muted)" }}>{label}</span>
      <span style={{ color: "var(--text-primary)", fontWeight: 600, textAlign: "right" }}>
        {value}
      </span>
    </div>
  );
}

function EmptyHint({ children }: { children: ReactNode }) {
  return (
    <div style={{ fontSize: 12.5, color: "var(--text-muted)", lineHeight: 1.5 }}>{children}</div>
  );
}

const tokenStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  padding: "5px 8px",
  borderRadius: 999,
  background: "color-mix(in srgb, var(--bg-panel) 92%, transparent)",
  border: "1px solid var(--border-dim)",
  color: "var(--text-secondary)",
  fontSize: 11.5,
  fontWeight: 600,
};

function severityPill(severity: string): CSSProperties {
  const lower = severity.toLowerCase();
  const accent =
    lower === "high" || lower === "error"
      ? "#ef4444"
      : lower === "medium" || lower === "warning"
        ? "#f59e0b"
        : "#22c55e";
  return {
    display: "inline-flex",
    alignItems: "center",
    padding: "3px 8px",
    borderRadius: 999,
    background: `${accent}1A`,
    color: accent,
    fontSize: 11,
    fontWeight: 700,
    textTransform: "uppercase",
  };
}
