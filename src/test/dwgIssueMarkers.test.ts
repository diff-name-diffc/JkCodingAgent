import { describe, expect, it } from "vitest";
import {
  buildCommandIssueMarkers,
  buildReviewIssueMarkers,
  mergeCommandIssueMarkers,
  resolveViewerIssueMarkers,
} from "../components/file-viewer/dwg/issueMarkers";
import type { CadReviewIssue, DwgIssueMarker } from "../types";

describe("dwg issue markers", () => {
  it("builds review markers and marks the active issue", () => {
    const issues: CadReviewIssue[] = [
      {
        id: "issue-1",
        runId: "run-1",
        severity: "high",
        title: "文字越界",
        description: "说明文字超出图框",
        entityRefs: [],
        anchorPoint: { x: 12, y: 24 },
        bbox: null,
        viewportHint: null,
        createdAt: "2026-04-22T00:00:00Z",
      },
      {
        id: "issue-2",
        runId: "run-1",
        severity: "medium",
        title: "尺寸断开",
        description: "尺寸线未闭合",
        entityRefs: [],
        anchorPoint: null,
        bbox: { minX: 0, minY: 0, maxX: 10, maxY: 20 },
        viewportHint: null,
        createdAt: "2026-04-22T00:00:00Z",
      },
    ];

    const markers = buildReviewIssueMarkers(issues, "issue-2");

    expect(markers).toHaveLength(2);
    expect(markers[0]).toMatchObject({ key: "review:issue-1", active: false });
    expect(markers[1]).toMatchObject({ key: "review:issue-2", active: true });
  });

  it("falls back to viewport hints when review issues do not carry direct anchors", () => {
    const markers = buildReviewIssueMarkers(
      [
        {
          id: "issue-viewport",
          runId: "run-1",
          severity: "medium",
          title: "局部空间不足",
          description: "当前问题只有视口提示",
          entityRefs: [],
          anchorPoint: null,
          bbox: null,
          viewportHint: {
            center: { x: 32, y: 48 },
            bbox: { minX: 20, minY: 30, maxX: 44, maxY: 60 },
            zoomScale: 6,
          },
          createdAt: "2026-04-22T00:00:00Z",
        },
      ],
      null,
    );

    expect(markers).toEqual([
      expect.objectContaining({
        key: "review:issue-viewport",
        anchorPoint: { x: 32, y: 48 },
        bbox: { minX: 20, minY: 30, maxX: 44, maxY: 60 },
      }),
    ]);
  });

  it("merges command markers by id and keeps the latest payload", () => {
    const existing: DwgIssueMarker[] = [
      { id: "m-1", severity: "low", anchorPoint: { x: 1, y: 2 } },
      { id: "m-2", severity: "medium", anchorPoint: { x: 3, y: 4 } },
    ];
    const incoming: DwgIssueMarker[] = [
      { id: "m-2", severity: "high", anchorPoint: { x: 30, y: 40 } },
      { id: "m-3", severity: "low", bbox: { minX: 5, minY: 6, maxX: 7, maxY: 8 } },
    ];

    expect(mergeCommandIssueMarkers(existing, incoming)).toEqual([
      { id: "m-1", severity: "low", anchorPoint: { x: 1, y: 2 } },
      { id: "m-2", severity: "high", anchorPoint: { x: 30, y: 40 } },
      { id: "m-3", severity: "low", bbox: { minX: 5, minY: 6, maxX: 7, maxY: 8 } },
    ]);
  });

  it("resolves marker targets from anchor points or bbox centers", () => {
    const markers = resolveViewerIssueMarkers(
      buildCommandIssueMarkers(
        [
          { id: "marker-1", anchorPoint: { x: 2, y: 4 } },
          { id: "marker-2", bbox: { minX: 10, minY: 20, maxX: 30, maxY: 60 } },
          { id: "marker-3" },
        ],
        "marker-2",
      ),
    );

    expect(markers).toHaveLength(2);
    expect(markers[0]).toMatchObject({
      key: "command:marker-1",
      target: { x: 2, y: 4 },
      active: false,
    });
    expect(markers[1]).toMatchObject({
      key: "command:marker-2",
      target: { x: 20, y: 40 },
      active: true,
    });
  });
});
