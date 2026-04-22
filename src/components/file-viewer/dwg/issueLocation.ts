import type { CadBBox, CadPoint, CadReviewIssue } from "../../../types";

export type CadIssueFocusPlan =
  | {
      kind: "fit_entities";
      entityIds: string[];
      bbox?: CadBBox | null;
      point?: CadPoint | null;
      zoomScale?: number | null;
    }
  | { kind: "fit_bbox"; bbox: CadBBox }
  | { kind: "fly_to_point"; point: CadPoint; zoomScale?: number | null }
  | { kind: "noop"; reason: string };

export function canLocateCadReviewIssue(issue: CadReviewIssue): boolean {
  return buildCadIssueFocusPlan(issue).kind !== "noop";
}

export function buildCadIssueFocusPlan(issue: CadReviewIssue): CadIssueFocusPlan {
  const viewportCenter = issue.viewportHint?.center ?? null;
  const viewportBox = issue.viewportHint?.bbox ?? null;
  const fallbackPoint = viewportCenter ?? issue.anchorPoint ?? bboxCenter(issue.bbox ?? null);
  const fallbackBox = viewportBox ?? issue.bbox ?? null;

  if (issue.entityRefs.length > 0) {
    return {
      kind: "fit_entities",
      entityIds: issue.entityRefs,
      bbox: fallbackBox,
      point: fallbackPoint,
      zoomScale: issue.viewportHint?.zoomScale ?? null,
    };
  }
  if (viewportCenter) {
    return {
      kind: "fly_to_point",
      point: viewportCenter,
      zoomScale: issue.viewportHint?.zoomScale ?? null,
    };
  }
  if (viewportBox) {
    return { kind: "fit_bbox", bbox: viewportBox };
  }
  if (issue.anchorPoint) {
    return { kind: "fly_to_point", point: issue.anchorPoint };
  }
  if (issue.bbox) {
    return { kind: "fit_bbox", bbox: issue.bbox };
  }
  return {
    kind: "noop",
    reason: "当前问题没有实体引用、视口提示、锚点或包围盒，无法定位到图纸。",
  };
}

function bboxCenter(bbox: CadBBox | null): CadPoint | null {
  if (!bbox) {
    return null;
  }
  return {
    x: (bbox.minX + bbox.maxX) / 2,
    y: (bbox.minY + bbox.maxY) / 2,
  };
}
