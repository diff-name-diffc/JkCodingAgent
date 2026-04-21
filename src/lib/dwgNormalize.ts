import type { DwgDatabase } from "@mlightcad/libredwg-web";
import type {
  CadBBox,
  CadEntityRecord,
  CadPoint,
  DwgBlockSummary,
  DwgLayerSummary,
  DwgParseSummary,
} from "../types";

function point(x: number, y: number): CadPoint {
  return { x, y };
}

function bboxFromPoints(points: CadPoint[]): CadBBox | null {
  if (points.length === 0) {
    return null;
  }

  let minX = points[0].x;
  let minY = points[0].y;
  let maxX = points[0].x;
  let maxY = points[0].y;

  for (const value of points.slice(1)) {
    minX = Math.min(minX, value.x);
    minY = Math.min(minY, value.y);
    maxX = Math.max(maxX, value.x);
    maxY = Math.max(maxY, value.y);
  }

  return { minX, minY, maxX, maxY };
}

function mergeBounds(current: CadBBox | null, next: CadBBox | null): CadBBox | null {
  if (!current) return next;
  if (!next) return current;
  return {
    minX: Math.min(current.minX, next.minX),
    minY: Math.min(current.minY, next.minY),
    maxX: Math.max(current.maxX, next.maxX),
    maxY: Math.max(current.maxY, next.maxY),
  };
}

function toPoint(value: { x: number; y: number } | { x: number; y: number; z: number }): CadPoint {
  return point(value.x, value.y);
}

function normalizeEntity(entity: Record<string, unknown>, index: number): CadEntityRecord {
  const entityType = String(entity.type ?? "UNKNOWN");
  const base: CadEntityRecord = {
    id: String(entity.handle ?? `${entityType}_${index}`),
    handle: String(entity.handle ?? `${entityType}_${index}`),
    entityType,
    rawType: entityType,
    layer: String(entity.layer ?? "0"),
    color: typeof entity.color === "number" ? entity.color : null,
    lineType: typeof entity.lineType === "string" ? entity.lineType : null,
    text: null,
    blockName: null,
    center: null,
    radius: null,
    vertices: [],
    bbox: null,
  };

  if (entityType === "LINE") {
    const startPoint = entity.startPoint as { x: number; y: number };
    const endPoint = entity.endPoint as { x: number; y: number };
    base.vertices = [toPoint(startPoint), toPoint(endPoint)];
    base.center = point((startPoint.x + endPoint.x) / 2, (startPoint.y + endPoint.y) / 2);
    base.bbox = bboxFromPoints(base.vertices);
    return base;
  }

  if (entityType === "LWPOLYLINE") {
    const vertices = Array.isArray(entity.vertices)
      ? (entity.vertices as Array<{ x: number; y: number }>).map(toPoint)
      : [];
    base.vertices = vertices;
    base.bbox = bboxFromPoints(vertices);
    base.center = base.bbox
      ? point((base.bbox.minX + base.bbox.maxX) / 2, (base.bbox.minY + base.bbox.maxY) / 2)
      : null;
    return base;
  }

  if (entityType === "CIRCLE" || entityType === "ARC") {
    const center = toPoint(entity.center as { x: number; y: number });
    const radius = typeof entity.radius === "number" ? entity.radius : null;
    base.center = center;
    base.radius = radius;
    if (radius !== null) {
      base.bbox = {
        minX: center.x - radius,
        minY: center.y - radius,
        maxX: center.x + radius,
        maxY: center.y + radius,
      };
    }
    return base;
  }

  if (entityType === "TEXT") {
    const startPoint = entity.startPoint as { x: number; y: number };
    const text = String(entity.text ?? "");
    base.text = text;
    base.center = toPoint(startPoint);
    base.bbox = bboxFromPoints([toPoint(startPoint)]);
    return base;
  }

  if (entityType === "MTEXT") {
    const insertionPoint = entity.insertionPoint as { x: number; y: number };
    const text = String(entity.text ?? "");
    const width = typeof entity.rectWidth === "number" ? entity.rectWidth : 0;
    const height = typeof entity.rectHeight === "number" ? entity.rectHeight : 0;
    base.text = text;
    base.center = toPoint(insertionPoint);
    base.bbox = {
      minX: insertionPoint.x,
      minY: insertionPoint.y - height,
      maxX: insertionPoint.x + width,
      maxY: insertionPoint.y,
    };
    return base;
  }

  if (entityType === "INSERT") {
    const insertionPoint = entity.insertionPoint as { x: number; y: number };
    base.blockName = String(entity.name ?? "");
    base.center = toPoint(insertionPoint);
    base.bbox = bboxFromPoints([toPoint(insertionPoint)]);
    return base;
  }

  if (entityType === "DIMENSION") {
    const textPoint = entity.textPoint as { x: number; y: number } | undefined;
    const definitionPoint = entity.definitionPoint as { x: number; y: number } | undefined;
    const anchor = textPoint ?? definitionPoint;
    base.text = typeof entity.text === "string" ? entity.text : null;
    if (anchor) {
      base.center = toPoint(anchor);
      base.bbox = bboxFromPoints([toPoint(anchor)]);
    }
    return base;
  }

  const fallbackCenter =
    entity.center && typeof entity.center === "object"
      ? toPoint(entity.center as { x: number; y: number })
      : null;
  base.center = fallbackCenter;
  base.bbox = fallbackCenter ? bboxFromPoints([fallbackCenter]) : null;
  return base;
}

export function buildNormalizedDwgIndex(
  database: DwgDatabase,
  filePath: string,
  parserVersion: string,
  unknownEntityCount = 0,
): { summary: DwgParseSummary; entities: CadEntityRecord[] } {
  const entities = database.entities.map((entity, index) =>
    normalizeEntity(entity as unknown as Record<string, unknown>, index),
  );

  const layerCount = new Map<string, number>();
  const entityCounts = new Map<string, number>();
  const textSamples = new Set<string>();
  const blockCounts = new Map<string, number>();
  let bounds: CadBBox | null = null;

  for (const entity of entities) {
    layerCount.set(entity.layer, (layerCount.get(entity.layer) ?? 0) + 1);
    entityCounts.set(entity.entityType, (entityCounts.get(entity.entityType) ?? 0) + 1);
    if (entity.text) {
      textSamples.add(entity.text.replace(/\s+/g, " ").trim().slice(0, 120));
    }
    if (entity.blockName) {
      blockCounts.set(entity.blockName, (blockCounts.get(entity.blockName) ?? 0) + 1);
    }
    bounds = mergeBounds(bounds, entity.bbox ?? null);
  }

  const layers: DwgLayerSummary[] = Array.from(layerCount.entries())
    .sort((left, right) => left[0].localeCompare(right[0]))
    .map(([name, entityCount]) => ({ name, entityCount }));
  const blocks: DwgBlockSummary[] = Array.from(blockCounts.entries())
    .sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]))
    .slice(0, 20)
    .map(([name, count]) => ({ name, count }));

  const summary: DwgParseSummary = {
    filePath,
    parserVersion,
    totalEntities: entities.length,
    unknownEntityCount,
    bounds,
    layers,
    entityCounts: Object.fromEntries(
      Array.from(entityCounts.entries()).sort((left, right) => left[0].localeCompare(right[0])),
    ),
    textSamples: Array.from(textSamples).filter(Boolean).slice(0, 20),
    blocks,
  };

  return { summary, entities };
}
