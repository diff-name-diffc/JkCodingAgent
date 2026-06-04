import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MarkdownCodeBlock } from "../components/markdown/MarkdownCodeBlock";
import { highlightCodeToHtml } from "../utils/shiki";

vi.mock("../utils/shiki", () => ({
  highlightCodeToHtml: vi.fn(() => Promise.reject(new Error("highlight failed"))),
}));

describe("MarkdownCodeBlock", () => {
  it("renders code text when syntax highlighting is unavailable", async () => {
    render(<MarkdownCodeBlock code="bunx ruff check ." language="bash" compact />);

    expect(screen.getByText("bash")).toBeInTheDocument();
    expect(screen.getByText("bunx ruff check .")).toBeInTheDocument();

    await waitFor(() =>
      expect(highlightCodeToHtml).toHaveBeenCalledWith("bunx ruff check .", "bash", false),
    );
    expect(screen.getByText("bunx ruff check .")).toBeInTheDocument();
  });

  it("escapes raw fallback code before injecting it into the DOM", () => {
    render(<MarkdownCodeBlock code="<script>alert(1)</script>" language="html" compact />);

    expect(screen.getByText("<script>alert(1)</script>")).toBeInTheDocument();
    expect(document.querySelector(".markdown-code-content script")).toBeNull();
  });

  it("shows run button only for persisted python code blocks with codeHash", () => {
    const onRunPython = vi.fn();
    render(
      <MarkdownCodeBlock
        code="print('hi')"
        language="python"
        messageId="msg-1"
        codeBlockIndex={2}
        codeHash="abc123"
        onRunPython={onRunPython}
        compact
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /run/i }));

    expect(onRunPython).toHaveBeenCalledWith({
      messageId: "msg-1",
      codeBlockIndex: 2,
      code: "print('hi')",
      codeHash: "abc123",
    });
  });

  it("does not show run button while streaming", () => {
    render(
      <MarkdownCodeBlock
        code="print('hi')"
        language="python"
        messageId="msg-1"
        codeBlockIndex={0}
        codeHash="abc"
        onRunPython={vi.fn()}
        compact
        streaming
      />,
    );

    expect(screen.queryByRole("button", { name: /run/i })).toBeNull();
  });

  it("shows inline output when runRecord is provided", () => {
    render(
      <MarkdownCodeBlock
        code="print('hello')"
        language="python"
        messageId="msg-1"
        codeBlockIndex={0}
        codeHash="xyz"
        runRecord={{
          runId: "r1",
          workspaceId: "ws1",
          messageId: "msg-1",
          codeBlockIndex: 0,
          codeHash: "xyz",
          code: "print('hello')",
          status: "done",
          stdout: "hello\n",
          stderr: "",
          installedPackagesJson: "[]",
          toolEventsJson: "[]",
          explanationMarkdown: "",
          createdAt: new Date().toISOString(),
          updatedAt: new Date().toISOString(),
        }}
        compact
      />,
    );

    expect(screen.getByText("hello")).toBeInTheDocument();
    expect(screen.getByText(/Done/)).toBeInTheDocument();
    // Run button should be hidden when record exists
    expect(screen.queryByRole("button", { name: /^run/i })).toBeNull();
  });

  it("shows running spinner when runRecord is running", () => {
    render(
      <MarkdownCodeBlock
        code="import time; time.sleep(10)"
        language="python"
        messageId="msg-1"
        codeBlockIndex={0}
        codeHash="abc"
        runRecord={{
          runId: "r2",
          workspaceId: "ws1",
          messageId: "msg-1",
          codeBlockIndex: 0,
          codeHash: "abc",
          code: "import time; time.sleep(10)",
          status: "running",
          stdout: "",
          stderr: "",
          installedPackagesJson: "[]",
          toolEventsJson: "[]",
          explanationMarkdown: "",
          createdAt: new Date().toISOString(),
          updatedAt: new Date().toISOString(),
        }}
        compact
      />,
    );

    expect(screen.getAllByText(/Running/).length).toBeGreaterThanOrEqual(1);
  });
});
