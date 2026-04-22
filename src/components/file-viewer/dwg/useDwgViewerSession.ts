import { useCallback, useEffect, useMemo, useRef, useState, type MutableRefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AcGeBox2d, AcGePoint2d } from "@mlightcad/data-model";
import {
  AcApContext,
  AcApDocument,
  AcApPanCmd,
  AcApSelectCmd,
  AcEdOpenMode,
  AcTrView2d,
} from "@mlightcad/cad-simple-viewer";
import type {
  CadBBox,
  CadPoint,
  CadReviewIssue,
  DwgViewerCommand,
  DwgViewerCommandResult,
  DwgViewerSessionRegistration,
  DwgViewerSessionState,
} from "../../../types";
import { openCadViewerDwgDocument } from "../../../lib/cadViewerDwg";

type ViewerBridge = {
  context: AcApContext;
  document: AcApDocument;
  view: AcTrView2d;
};

export function useDwgViewerSession({
  tabId,
  active,
  filePath,
  fileName,
  workspaceId,
  isDark,
  bytes,
  parseStatus,
  docId,
  parseError,
  activeIssue,
}: {
  tabId: string;
  active: boolean;
  filePath: string;
  fileName: string;
  workspaceId: string | null;
  isDark: boolean;
  bytes: Uint8Array | null;
  parseStatus: string;
  docId: string | null;
  parseError: string | null;
  activeIssue: CadReviewIssue | null;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const markerRef = useRef<HTMLDivElement | null>(null);
  const markerFrameRef = useRef<number | null>(null);
  const viewerRef = useRef<ViewerBridge | null>(null);
  const commandQueueRef = useRef<Promise<void>>(Promise.resolve());
  const sessionId = useMemo(
    () => (workspaceId ? `dwg-${workspaceId}-${tabId}` : null),
    [tabId, workspaceId],
  );
  const [loadingViewer, setLoadingViewer] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState<"select" | "pan">("select");
  const [viewerNotice, setViewerNotice] = useState<string | null>(null);

  const buildStateSnapshot = useCallback((): DwgViewerSessionRegistration | null => {
    const bridge = viewerRef.current;
    if (!sessionId || !workspaceId) {
      return null;
    }
    return {
      sessionId,
      workspaceId,
      filePath,
      tabId,
      visible: active,
      active,
      mode: viewMode,
      parseStatus,
      canvasWidth: bridge?.view.width ?? 0,
      canvasHeight: bridge?.view.height ?? 0,
      viewportBox: bridge ? getViewportBox(bridge.view) : null,
      center: bridge ? pointToCadPoint(bridge.view.center) : null,
      zoomScale: bridge ? bridge.view.activeLayoutView.internalCamera.zoom : null,
      selectionIds: bridge ? [...bridge.view.selectionSet.ids] : [],
      docId,
      parseError,
    };
  }, [active, docId, filePath, parseError, parseStatus, sessionId, tabId, viewMode, workspaceId]);

  const syncViewerState = useCallback(async () => {
    const payload = buildStateSnapshot();
    if (!payload || !workspaceId) {
      return;
    }
    try {
      if (viewerRef.current) {
        await invoke<DwgViewerSessionState>("dispatcher_update_dwg_viewer_state", { payload });
      }
    } catch (nextError) {
      console.error("同步 DWG viewer 状态失败:", nextError);
    }
  }, [buildStateSnapshot, workspaceId]);

  useEffect(() => {
    let cancelled = false;

    async function boot() {
      if (!bytes) {
        setLoadingViewer(false);
        return;
      }
      setLoadingViewer(true);
      setError(null);
      try {
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
        const modelSpaceBtrId =
          document.database.tables.blockTable.modelSpace.objectId || document.database.currentSpaceId;
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
        await new AcApSelectCmd().execute(context).catch(() => undefined);
        setViewMode("select");
        if (!cancelled) {
          setLoadingViewer(false);
        }
      } catch (nextError) {
        if (!cancelled) {
          setError(nextError instanceof Error ? nextError.message : String(nextError));
          setLoadingViewer(false);
        }
      }
    }

    void boot();
    return () => {
      cancelled = true;
      if (markerFrameRef.current !== null) {
        cancelAnimationFrame(markerFrameRef.current);
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
  }, [bytes, fileName, isDark]);

  useEffect(() => {
    if (!sessionId || !workspaceId) {
      return;
    }

    void invoke<DwgViewerSessionState>("dispatcher_register_dwg_viewer_session", {
      payload: buildStateSnapshot(),
    }).catch((nextError) => {
      console.error("注册 DWG viewer 会话失败:", nextError);
    });

    return () => {
      if (sessionId) {
        void invoke("dispatcher_unregister_dwg_viewer_session", { sessionId }).catch(console.error);
      }
    };
  }, [buildStateSnapshot, sessionId, workspaceId]);

  useEffect(() => {
    if (!viewerRef.current || !sessionId || !workspaceId) {
      return;
    }
    let frameTimer: ReturnType<typeof setTimeout> | null = null;
    const bridge = viewerRef.current;

    const scheduleSync = () => {
      if (frameTimer) {
        clearTimeout(frameTimer);
      }
      frameTimer = setTimeout(() => {
        void syncViewerState();
      }, 100);
    };

    const selectionAdded = () => scheduleSync();
    const selectionRemoved = () => scheduleSync();
    const viewChanged = () => scheduleSync();

    bridge.view.selectionSet.events.selectionAdded.addEventListener(selectionAdded);
    bridge.view.selectionSet.events.selectionRemoved.addEventListener(selectionRemoved);
    bridge.view.events.viewChanged.addEventListener(viewChanged);
    void syncViewerState();

    return () => {
      if (frameTimer) {
        clearTimeout(frameTimer);
      }
      bridge.view.selectionSet.events.selectionAdded.removeEventListener(selectionAdded);
      bridge.view.selectionSet.events.selectionRemoved.removeEventListener(selectionRemoved);
      bridge.view.events.viewChanged.removeEventListener(viewChanged);
    };
  }, [loadingViewer, sessionId, syncViewerState, workspaceId]);

  useEffect(() => {
    if (!viewerRef.current || !sessionId || !workspaceId) {
      return;
    }
    void syncViewerState();
  }, [active, docId, parseError, parseStatus, sessionId, syncViewerState, viewMode, workspaceId]);

  useEffect(() => {
    if (!sessionId) {
      return;
    }
    let disposed = false;
    let unsubscribe: (() => void) | null = null;

    const resolveCommand = async (payload: DwgViewerCommandResult) => {
      try {
        await invoke("dispatcher_resolve_dwg_viewer_command", { payload });
      } catch (nextError) {
        console.error("回传 DWG viewer 命令结果失败:", nextError);
      }
    };

    void listen<DwgViewerCommand>("dwg-viewer/command", (event) => {
      if (disposed) return;
      if (event.payload.sessionId !== sessionId) {
        return;
      }
      const runCommand = async () => {
        const result = await executeCommand(event.payload, viewerRef.current, setViewMode);
        await resolveCommand(result);
        await syncViewerState();
      };
      commandQueueRef.current = commandQueueRef.current.catch(() => undefined).then(runCommand);
    }).then((off) => {
      unsubscribe = off;
    });

    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, [sessionId]);

  useEffect(() => {
    const bridge = viewerRef.current;
    if (!bridge || !activeIssue) {
      bridge?.view.selectionSet.clear();
      setViewerNotice(null);
      clearMarker(markerFrameRef, markerRef);
      return;
    }

    const cleanup = locateCadIssueInViewer(
      bridge,
      activeIssue,
      markerRef,
      markerFrameRef,
      setViewerNotice,
    );
    void syncViewerState();
    return cleanup;
  }, [activeIssue, syncViewerState]);

  const switchToSelect = useCallback(() => {
    const bridge = viewerRef.current;
    if (!bridge) return;
    void new AcApSelectCmd().execute(bridge.context).catch(() => undefined);
    setViewMode("select");
  }, []);

  const switchToPan = useCallback(() => {
    const bridge = viewerRef.current;
    if (!bridge) return;
    void new AcApPanCmd().execute(bridge.context).catch(() => undefined);
    setViewMode("pan");
  }, []);

  return {
    containerRef,
    loadingViewer,
    error,
    viewMode,
    viewerNotice,
    switchToSelect,
    switchToPan,
  };
}

async function executeCommand(
  command: DwgViewerCommand,
  bridge: ViewerBridge | null,
  setViewMode: (mode: "select" | "pan") => void,
): Promise<DwgViewerCommandResult> {
  if (!bridge) {
    return {
      commandId: command.commandId,
      sessionId: command.sessionId,
      ok: false,
      error: "DWG viewer 尚未准备好",
    };
  }
  try {
    const payload = command.payload as Record<string, unknown>;
    let result: unknown = null;
    switch (command.action) {
      case "fit_drawing":
        bridge.view.zoomToFitDrawing(1500);
        break;
      case "fit_bbox": {
        const bbox = payload.bbox as CadBBox | undefined;
        if (!bbox) {
          throw new Error("fit_bbox 缺少 bbox");
        }
        bridge.view.zoomTo(
          new AcGeBox2d(
            new AcGePoint2d(bbox.minX, bbox.minY),
            new AcGePoint2d(bbox.maxX, bbox.maxY),
          ),
        );
        break;
      }
      case "fit_entities":
      case "select_entities": {
        const entityIds = Array.isArray(payload.entityIds)
          ? payload.entityIds.filter((value): value is string => typeof value === "string")
          : [];
        bridge.view.selectionSet.clear();
        if (entityIds.length > 0) {
          bridge.view.selectionSet.add(entityIds);
          bridge.view.highlight(entityIds);
        }
        const bbox = payload.bbox as CadBBox | undefined;
        if (bbox) {
          bridge.view.zoomTo(
            new AcGeBox2d(
              new AcGePoint2d(bbox.minX, bbox.minY),
              new AcGePoint2d(bbox.maxX, bbox.maxY),
            ),
          );
        }
        result = { entityIds };
        break;
      }
      case "fly_to_point": {
        const point = payload.point as CadPoint | undefined;
        if (!point) {
          throw new Error("fly_to_point 缺少 point");
        }
        bridge.view.flyTo(point, typeof payload.zoomScale === "number" ? Number(payload.zoomScale) : 4);
        break;
      }
      case "zoom_by_factor": {
        const factor = typeof payload.factor === "number" ? Number(payload.factor) : 1;
        const box = getViewportBox(bridge.view);
        if (!box || factor <= 0) {
          break;
        }
        const cx = (box.minX + box.maxX) / 2;
        const cy = (box.minY + box.maxY) / 2;
        const halfW = (box.maxX - box.minX) / 2 / factor;
        const halfH = (box.maxY - box.minY) / 2 / factor;
        bridge.view.zoomTo(
          new AcGeBox2d(
            new AcGePoint2d(cx - halfW, cy - halfH),
            new AcGePoint2d(cx + halfW, cy + halfH),
          ),
        );
        break;
      }
      case "pan_by_view_ratio": {
        const box = getViewportBox(bridge.view);
        if (!box) break;
        const dxRatio = typeof payload.dxRatio === "number" ? Number(payload.dxRatio) : 0;
        const dyRatio = typeof payload.dyRatio === "number" ? Number(payload.dyRatio) : 0;
        bridge.view.flyTo(
          {
            x: bridge.view.center.x + (box.maxX - box.minX) * dxRatio,
            y: bridge.view.center.y + (box.maxY - box.minY) * dyRatio,
          },
          bridge.view.activeLayoutView.internalCamera.zoom,
        );
        break;
      }
      case "clear_selection":
        bridge.view.selectionSet.clear();
        break;
      case "set_mode":
        if (payload.mode === "pan") {
          await new AcApPanCmd().execute(bridge.context).catch(() => undefined);
          setViewMode("pan");
        } else {
          await new AcApSelectCmd().execute(bridge.context).catch(() => undefined);
          setViewMode("select");
        }
        break;
      case "pick": {
        const hitRadius = typeof payload.hitRadius === "number" ? Number(payload.hitRadius) : undefined;
        const pickOneOnly =
          typeof payload.pickOneOnly === "boolean" ? Boolean(payload.pickOneOnly) : true;
        const worldPoint = payload.worldPoint as CadPoint | undefined;
        const screenPoint = payload.screenPoint as CadPoint | undefined;
        const point = worldPoint
          ? worldPoint
          : screenPoint
            ? bridge.view.screenToWorld(screenPoint)
            : undefined;
        result = bridge.view.pick(point, hitRadius, pickOneOnly);
        break;
      }
      case "capture": {
        const dataUrl = bridge.view.canvas.toDataURL("image/png");
        result = {
          dataUrl,
          width: bridge.view.canvas.width,
          height: bridge.view.canvas.height,
          viewportBox: getViewportBox(bridge.view),
        };
        break;
      }
      case "noop":
      default:
        result = {
          viewportBox: getViewportBox(bridge.view),
          selectionIds: [...bridge.view.selectionSet.ids],
        };
        break;
    }
    return {
      commandId: command.commandId,
      sessionId: command.sessionId,
      ok: true,
      result,
    };
  } catch (error) {
    return {
      commandId: command.commandId,
      sessionId: command.sessionId,
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

function getViewerContainerSize(container: HTMLDivElement) {
  const rect = container.getBoundingClientRect();
  return {
    width: Math.max(1, Math.floor(rect.width)),
    height: Math.max(1, Math.floor(rect.height)),
  };
}

function getViewportBox(view: AcTrView2d): CadBBox | null {
  const topLeft = view.screenToWorld({ x: 0, y: 0 });
  const bottomRight = view.screenToWorld({ x: view.width, y: view.height });
  return {
    minX: Math.min(topLeft.x, bottomRight.x),
    minY: Math.min(topLeft.y, bottomRight.y),
    maxX: Math.max(topLeft.x, bottomRight.x),
    maxY: Math.max(topLeft.y, bottomRight.y),
  };
}

function pointToCadPoint(point: { x: number; y: number } | null | undefined) {
  if (!point) return null;
  return { x: point.x, y: point.y };
}

function bboxCenter(bbox: CadBBox | null): CadPoint | null {
  if (!bbox) return null;
  return {
    x: (bbox.minX + bbox.maxX) / 2,
    y: (bbox.minY + bbox.maxY) / 2,
  };
}

function clearMarker(
  markerFrameRef: MutableRefObject<number | null>,
  markerRef: MutableRefObject<HTMLDivElement | null>,
) {
  if (markerFrameRef.current !== null) {
    cancelAnimationFrame(markerFrameRef.current);
    markerFrameRef.current = null;
  }
  markerRef.current?.remove();
  markerRef.current = null;
}

function mountMarkerLoop(
  view: AcTrView2d,
  target: CadPoint,
  markerRef: MutableRefObject<HTMLDivElement | null>,
  markerFrameRef: MutableRefObject<number | null>,
) {
  clearMarker(markerFrameRef, markerRef);
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

function locateCadIssueInViewer(
  bridge: ViewerBridge,
  issue: CadReviewIssue,
  markerRef: MutableRefObject<HTMLDivElement | null>,
  markerFrameRef: MutableRefObject<number | null>,
  setViewerNotice: (message: string | null) => void,
) {
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
    } catch (nextError) {
      setViewerNotice(nextError instanceof Error ? nextError.message : "问题图元高亮失败");
    }
  }

  if (target) {
    void new AcApSelectCmd().execute(bridge.context).catch(() => undefined);
    bridge.view.flyTo(target, 4);
    mountMarkerLoop(bridge.view, target, markerRef, markerFrameRef);
  } else {
    clearMarker(markerFrameRef, markerRef);
  }

  return () => {
    if (ids.length > 0) {
      bridge.view.unhighlight(ids);
    }
  };
}
