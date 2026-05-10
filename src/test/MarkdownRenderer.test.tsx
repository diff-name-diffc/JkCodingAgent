import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { MarkdownRenderer } from "../components/markdown/MarkdownRenderer";

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
});
