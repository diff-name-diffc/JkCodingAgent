import type { AcTrView2d } from "@mlightcad/cad-simple-viewer";
import type { CadBBox, CadPoint, CadReviewIssue, DwgIssueMarker } from "../../../types";

const MARKER_SIZE_PX = 18;
const MARKER_HALF_SIZE_PX = MARKER_SIZE_PX / 2;
const VIEWPORT_PADDING_RATIO = 0.04;

export interface ViewerIssueMarkerDefinition {
  key: string;
  id: string;
  severity?: string | null;
  title?: string | null;
  anchorPoint?: CadPoint | null;
  bbox?: CadBBox | null;
  active?: boolean;
}

interface ResolvedViewerIssueMarker extends ViewerIssueMarkerDefinition {
  target: CadPoint;
}

export interface IssueMarkerLayerHandle {
  dispose: () => void;
  scheduleRefresh: () => void;
  setMarkers: (markers: ViewerIssueMarkerDefinition[]) => void;
}

export function buildReviewIssueMarkers(
  issues: CadReviewIssue[],
  activeIssueId: string | null,
): ViewerIssueMarkerDefinition[] {
  return issues.map((issue) => ({
    key: `review:${issue.id}`,
    id: issue.id,
    severity: issue.severity,
    title: issue.title,
    anchorPoint: issue.anchorPoint ?? issue.viewportHint?.center ?? null,
    bbox: issue.bbox ?? issue.viewportHint?.bbox ?? null,
    active: issue.id === activeIssueId,
  }));
}

export function buildCommandIssueMarkers(
  markers: DwgIssueMarker[],
  activeMarkerId: string | null,
): ViewerIssueMarkerDefinition[] {
  return markers.map((marker) => ({
    key: `command:${marker.id}`,
    id: marker.id,
    severity: marker.severity,
    title: marker.title,
    anchorPoint: marker.anchorPoint,
    bbox: marker.bbox,
    active: marker.id === activeMarkerId,
  }));
}

export function mergeCommandIssueMarkers(
  existing: DwgIssueMarker[],
  incoming: DwgIssueMarker[],
): DwgIssueMarker[] {
  const next = new Map(existing.map((marker) => [marker.id, marker]));
  for (const marker of incoming) {
    next.set(marker.id, marker);
  }
  return [...next.values()];
}

export function resolveViewerIssueMarkers(
  markers: ViewerIssueMarkerDefinition[],
): ResolvedViewerIssueMarker[] {
  return markers
    .map((marker) => {
      const target = resolveIssueMarkerTarget(marker.anchorPoint ?? null, marker.bbox ?? null);
      if (!target) {
        return null;
      }
      return {
        ...marker,
        target,
      };
    })
    .filter((marker): marker is ResolvedViewerIssueMarker => marker !== null);
}

export function resolveIssueMarkerTarget(
  anchorPoint?: CadPoint | null,
  bbox?: CadBBox | null,
): CadPoint | null {
  return anchorPoint ?? bboxCenter(bbox ?? null);
}

export function bboxCenter(bbox: CadBBox | null): CadPoint | null {
  if (!bbox) {
    return null;
  }
  return {
    x: (bbox.minX + bbox.maxX) / 2,
    y: (bbox.minY + bbox.maxY) / 2,
  };
}

export function getViewportBox(view: AcTrView2d): CadBBox | null {
  const topLeft = view.screenToWorld({ x: 0, y: 0 });
  const bottomRight = view.screenToWorld({ x: view.width, y: view.height });
  return {
    minX: Math.min(topLeft.x, bottomRight.x),
    minY: Math.min(topLeft.y, bottomRight.y),
    maxX: Math.max(topLeft.x, bottomRight.x),
    maxY: Math.max(topLeft.y, bottomRight.y),
  };
}

export function createIssueMarkerLayer(view: AcTrView2d): IssueMarkerLayerHandle {
  const overlay = document.createElement("div");
  overlay.style.position = "absolute";
  overlay.style.inset = "0";
  overlay.style.pointerEvents = "none";
  overlay.style.overflow = "hidden";
  overlay.style.zIndex = "35";
  view.container.appendChild(overlay);

  const markerElements = new Map<string, HTMLDivElement>();
  let markers: ResolvedViewerIssueMarker[] = [];
  let frameId: number | null = null;
  let disposed = false;

  const resizeObserver =
    typeof ResizeObserver !== "undefined"
      ? new ResizeObserver(() => {
          scheduleRefresh();
        })
      : null;
  resizeObserver?.observe(view.container);

  const syncMarkerElement = (marker: ResolvedViewerIssueMarker, element: HTMLDivElement) => {
    const palette = markerPalette(marker.severity, Boolean(marker.active));
    element.style.borderColor = palette.borderColor;
    element.style.boxShadow = palette.boxShadow;
    element.style.opacity = marker.active ? "1" : "0.88";
    element.style.zIndex = marker.active ? "2" : "1";
    element.dataset.active = marker.active ? "true" : "false";
    if (marker.title) {
      element.setAttribute("aria-label", marker.title);
      element.title = marker.title;
    } else {
      element.removeAttribute("aria-label");
      element.removeAttribute("title");
    }
  };

  const ensureMarkerElement = (marker: ResolvedViewerIssueMarker) => {
    const existing = markerElements.get(marker.key);
    if (existing) {
      syncMarkerElement(marker, existing);
      return existing;
    }
    const element = document.createElement("div");
    element.style.position = "absolute";
    element.style.width = `${MARKER_SIZE_PX}px`;
    element.style.height = `${MARKER_SIZE_PX}px`;
    element.style.borderRadius = "999px";
    element.style.border = "3px solid transparent";
    element.style.background = "transparent";
    element.style.pointerEvents = "none";
    element.style.willChange = "transform";
    syncMarkerElement(marker, element);
    overlay.appendChild(element);
    markerElements.set(marker.key, element);
    return element;
  };

  const refresh = () => {
    frameId = null;
    if (disposed) {
      return;
    }
    const viewport = getViewportBox(view);
    const visibleViewport = viewport ? inflateBBox(viewport, VIEWPORT_PADDING_RATIO) : null;
    const activeKeys = new Set(markers.map((marker) => marker.key));

    for (const marker of markers) {
      const element = ensureMarkerElement(marker);
      if (visibleViewport && !pointInBBox(marker.target, visibleViewport)) {
        element.style.display = "none";
        continue;
      }
      const screenPoint = view.worldToScreen(marker.target);
      const containerPoint = view.canvasToContainer(screenPoint);
      element.style.display = "block";
      element.style.transform = `translate(${containerPoint.x - MARKER_HALF_SIZE_PX}px, ${
        containerPoint.y - MARKER_HALF_SIZE_PX
      }px)`;
    }

    for (const [key, element] of markerElements) {
      if (activeKeys.has(key)) {
        continue;
      }
      element.remove();
      markerElements.delete(key);
    }
  };

  const scheduleRefresh = () => {
    if (disposed || frameId !== null) {
      return;
    }
    frameId = requestAnimationFrame(refresh);
  };

  return {
    dispose() {
      disposed = true;
      if (frameId !== null) {
        cancelAnimationFrame(frameId);
      }
      resizeObserver?.disconnect();
      markerElements.clear();
      overlay.remove();
    },
    scheduleRefresh,
    setMarkers(nextMarkers) {
      markers = resolveViewerIssueMarkers(nextMarkers);
      scheduleRefresh();
    },
  };
}

function inflateBBox(bbox: CadBBox, paddingRatio: number): CadBBox {
  const width = bbox.maxX - bbox.minX;
  const height = bbox.maxY - bbox.minY;
  const padding = Math.max(width, height, 1) * paddingRatio;
  return {
    minX: bbox.minX - padding,
    minY: bbox.minY - padding,
    maxX: bbox.maxX + padding,
    maxY: bbox.maxY + padding,
  };
}

function pointInBBox(point: CadPoint, bbox: CadBBox): boolean {
  return (
    point.x >= bbox.minX && point.x <= bbox.maxX && point.y >= bbox.minY && point.y <= bbox.maxY
  );
}

function markerPalette(severity: string | null | undefined, active: boolean) {
  const normalized = severity?.toLowerCase();
  if (normalized === "high" || normalized === "error" || normalized === "critical") {
    return {
      borderColor: active ? "#dc2626" : "#ef4444",
      boxShadow: active ? "0 0 0 7px rgba(239,68,68,0.18)" : "0 0 0 5px rgba(239,68,68,0.1)",
    };
  }
  if (normalized === "medium" || normalized === "warning") {
    return {
      borderColor: active ? "#d97706" : "#f59e0b",
      boxShadow: active ? "0 0 0 7px rgba(245,158,11,0.18)" : "0 0 0 5px rgba(245,158,11,0.1)",
    };
  }
  return {
    borderColor: active ? "#2563eb" : "#3b82f6",
    boxShadow: active ? "0 0 0 7px rgba(59,130,246,0.18)" : "0 0 0 5px rgba(59,130,246,0.1)",
  };
}
