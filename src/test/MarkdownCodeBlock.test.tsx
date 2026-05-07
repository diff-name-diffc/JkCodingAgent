import { render, screen, waitFor } from "@testing-library/react";
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
});
