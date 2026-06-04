import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MarkdownRenderer } from "../components/markdown/MarkdownRenderer";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string, payload?: Record<string, unknown>) => {
    if (cmd === "resolve_chat_image") {
      return {
        imageId: payload?.imageId ?? "unknown",
        path: `/resolved/${payload?.imageId ?? "unknown"}.png`,
        mimeType: "image/png",
      };
    }
    if (cmd === "convertFileSrc" || cmd === "assetPath") {
      return `http://asset.localhost/${payload?.path ?? ""}`;
    }
    return null;
  }),
  convertFileSrc: (path: string) => `http://asset.localhost/${path}`,
  Channel: class {},
}));

describe("MarkdownRenderer", () => {
  it("renders block and inline math expressions", () => {
    const content = [
      "Softmax 的公式为：",
      "",
      "$$\\text{softmax}(x_i) = \\frac{e^{x_i}}{\\sum_{j=1}^{n} e^{x_j}}$$",
      "",
      "其中 $x_i$ 是第 $i$ 个位置的原始分数，$n$ 是向量长度。",
    ].join("\n");

    const { container } = render(<MarkdownRenderer content={content} variant="chat" />);

    expect(screen.getByText("Softmax 的公式为：")).toBeInTheDocument();
    expect(container.querySelector(".katex-display")).toBeInTheDocument();
    expect(container.querySelectorAll(".katex").length).toBeGreaterThanOrEqual(3);
    expect(container).not.toHaveTextContent("$$");
  });

  it("assigns python code block indexes by markdown order", () => {
    const onRunPython = vi.fn();
    const content = [
      "```python",
      "print('a')",
      "```",
      "",
      "```bash",
      "echo ignored",
      "```",
      "",
      "```py",
      "print('b')",
      "```",
    ].join("\n");

    render(
      <MarkdownRenderer
        content={content}
        variant="chat"
        messageId="msg-1"
        onRunPython={onRunPython}
      />,
    );

    const runButtons = screen.getAllByRole("button", { name: /run/i });
    fireEvent.click(runButtons[0]);
    fireEvent.click(runButtons[1]);

    const firstCall = onRunPython.mock.calls[0][0];
    expect(firstCall.messageId).toBe("msg-1");
    expect(firstCall.codeBlockIndex).toBe(0);
    expect(firstCall.code).toBe("print('a')");
    expect(typeof firstCall.codeHash).toBe("string");

    const secondCall = onRunPython.mock.calls[1][0];
    expect(secondCall.messageId).toBe("msg-1");
    expect(secondCall.codeBlockIndex).toBe(2);
    expect(secondCall.code).toBe("print('b')");
    expect(typeof secondCall.codeHash).toBe("string");
  });

  it("passes runRecord to matching code block via pythonRunRecords", () => {
    const content = "```python\nprint('hello')\n```";

    // Compute the same hash as stableHash("print('hello')")
    function stableHash(text: string): string {
      let h = 0x811c9dc5;
      for (let i = 0; i < text.length; i++) {
        h ^= text.charCodeAt(i);
        h = (h * 0x01000193) >>> 0;
      }
      return h.toString(36);
    }
    const hash = stableHash("print('hello')");

    const records: Record<string, { status: string; stdout: string; stderr: string; codeHash: string; runId: string; workspaceId: string; messageId: string; codeBlockIndex: number; code: string; installedPackagesJson: string; toolEventsJson: string; explanationMarkdown: string; createdAt: string; updatedAt: string }> = {
      [`msg-1:${hash}`]: {
        runId: "r1",
        workspaceId: "ws1",
        messageId: "msg-1",
        codeBlockIndex: 0,
        codeHash: hash,
        code: "print('hello')",
        status: "done",
        stdout: "hello\n",
        stderr: "",
        installedPackagesJson: "[]",
        toolEventsJson: "[]",
        explanationMarkdown: "",
        createdAt: new Date().toISOString(),
        updatedAt: new Date().toISOString(),
      },
    };

    render(
      <MarkdownRenderer
        content={content}
        variant="chat"
        messageId="msg-1"
        pythonRunRecords={records}
      />,
    );

    // Should show inline output
    expect(screen.getByText("hello")).toBeInTheDocument();
    expect(screen.getByText(/Done/)).toBeInTheDocument();
  });

  it("renders chat-image:// protocol images (URL not stripped by sanitizer)", async () => {
    const content = "![test image](chat-image://abc-123-uuid)";

    const { container } = render(<MarkdownRenderer content={content} variant="chat" />);

    // MarkdownImage renders a loading state ("加载中...") while awaiting invoke;
    // either the loading text or a resolved <img> must be present (previous bug:
    // rehype/react-markdown stripped the custom protocol, producing no image element).
    await waitFor(() => {
      const hasImg = container.querySelector("img");
      const hasLoading = screen.queryByText("加载中...");
      if (!hasImg && !hasLoading) {
        throw new Error(`No image rendered. HTML: ${container.innerHTML}`);
      }
    });
  });
});
