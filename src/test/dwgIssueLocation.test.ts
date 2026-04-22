import { describe, expect, it } from "vitest";
import {
  buildCadIssueFocusPlan,
  canLocateCadReviewIssue,
} from "../components/file-viewer/dwg/issueLocation";
import type { CadReviewIssue } from "../types";

function createIssue(overrides: Partial<CadReviewIssue>): CadReviewIssue {
  return {
    id: "issue-1",
    runId: "run-1",
    severity: "high",
    title: "示例问题",
    description: "示例描述",
    entityRefs: [],
    anchorPoint: null,
    bbox: null,
    viewportHint: null,
    ruleRef: null,
    createdAt: "2026-04-22T00:00:00Z",
    ...overrides,
  };
}

describe("dwg issue location", () => {
  it("prefers entity refs and carries bbox fallback for fit actions", () => {
    const issue = createIssue({
      entityRefs: ["L1", "L2"],
      viewportHint: {
        center: { x: 12, y: 18 },
        bbox: { minX: 10, minY: 14, maxX: 20, maxY: 26 },
        zoomScale: 5,
      },
    });

    expect(buildCadIssueFocusPlan(issue)).toEqual({
      kind: "fit_entities",
      entityIds: ["L1", "L2"],
      bbox: { minX: 10, minY: 14, maxX: 20, maxY: 26 },
      point: { x: 12, y: 18 },
      zoomScale: 5,
    });
  });

  it("falls back to viewport center before anchor and bbox", () => {
    const issue = createIssue({
      viewportHint: {
        center: { x: 30, y: 42 },
        bbox: { minX: 20, minY: 32, maxX: 40, maxY: 52 },
        zoomScale: 7,
      },
      anchorPoint: { x: 1, y: 2 },
      bbox: { minX: 0, minY: 0, maxX: 4, maxY: 6 },
    });

    expect(buildCadIssueFocusPlan(issue)).toEqual({
      kind: "fly_to_point",
      point: { x: 30, y: 42 },
      zoomScale: 7,
    });
  });

  it("reports noop when an issue has no locatable geometry", () => {
    const issue = createIssue({});

    expect(canLocateCadReviewIssue(issue)).toBe(false);
    expect(buildCadIssueFocusPlan(issue)).toEqual({
      kind: "noop",
      reason: "当前问题没有实体引用、视口提示、锚点或包围盒，无法定位到图纸。",
    });
  });
});
