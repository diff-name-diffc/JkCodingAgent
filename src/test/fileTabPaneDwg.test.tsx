import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../components/file-viewer/MonacoEditorPane", () => ({
  MonacoEditorPane: () => <div data-testid="monaco-editor" />,
}));

vi.mock("../components/file-viewer/LargeFileViewer", () => ({
  LargeFileViewer: () => <div data-testid="large-file-viewer" />,
}));

vi.mock("../components/file-viewer/DwgWorkbenchPane", () => ({
  DwgWorkbenchPane: ({ filePath }: { filePath: string }) => (
    <div data-testid="dwg-workbench">DWG:{filePath}</div>
  ),
}));

import { FileTabPane } from "../components/file-viewer/FileTabPane";

describe("FileTabPane", () => {
  it("打开 .dwg 文件时渲染 DwgWorkbenchPane", () => {
    render(
      <FileTabPane
        active
        tab={{ id: "tab-1", path: "/repo/sample.dwg", name: "sample.dwg" }}
        projectPath="/repo"
        isDark={false}
        workspaceId="ws-1"
        activeCadReviewRunId={null}
        activeCadIssueId={null}
        onActiveCadReviewRunChange={() => undefined}
        onActiveCadIssueChange={() => undefined}
      />,
    );

    expect(screen.getByTestId("dwg-workbench")).toHaveTextContent("DWG:/repo/sample.dwg");
  });
});
