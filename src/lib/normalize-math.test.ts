import { describe, expect, it } from "vitest";
import {
  needsMathNormalize,
  normalizeLatexMathDelimiters,
  normalizeMarkdownMath,
  normalizeMathCodeFences,
  normalizeSingleLineMathBlocks,
} from "./normalize-math";

describe("normalizeMathCodeFences", () => {
  it("rewrites explicit math-language fences to display math", () => {
    const input = "before\n\n```math\n∑_{t=1}^{n} O(t²) = O(n³)\n```\n\nafter";
    expect(normalizeMathCodeFences(input)).toBe(
      "before\n\n$$\n∑_{t=1}^{n} O(t²) = O(n³)\n$$\n\nafter",
    );
  });

  it("rewrites latex/tex tagged fences", () => {
    expect(normalizeMathCodeFences("```latex\n\\frac{a}{b}\n```")).toBe(
      "$$\n\\frac{a}{b}\n$$",
    );
  });

  it("sniffs untagged fences with unicode math signals", () => {
    const line = "∑_{t=1}^{n} O(t²) = O(1² + 2² + ... + n²) = O(n³/3) ≈ O(n³)";
    expect(normalizeMathCodeFences(`\`\`\`\n${line}\n\`\`\``)).toBe(`$$\n${line}\n$$`);
  });

  it("sniffs pure-ASCII LaTeX via command + subscript structure", () => {
    const input = "```\n\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}\n```";
    expect(normalizeMathCodeFences(input)).toBe(
      "$$\n\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}\n$$",
    );
  });

  it("leaves untagged real code untouched (single weak signal)", () => {
    const input = "```\nconst mask = a^b;\nlet x_1 = 2;\n```";
    expect(normalizeMathCodeFences(input)).toBe(input);
  });

  it("leaves language-tagged code untouched even with math-ish tokens", () => {
    const input = "```python\ntotal = sum(x^2 for x in xs)\n```";
    expect(normalizeMathCodeFences(input)).toBe(input);
  });

  it("leaves long untagged blocks untouched", () => {
    const body = Array.from({ length: 13 }, (_, i) => `∑_{i} x² line ${i}`).join("\n");
    const input = "```\n" + body + "\n```";
    expect(normalizeMathCodeFences(input)).toBe(input);
  });

  it("leaves unterminated fences untouched while streaming", () => {
    const input = "```math\n\\sum_{i=1}^{n}";
    expect(normalizeMathCodeFences(input)).toBe(input);
  });

  it("leaves empty math-tagged fences untouched", () => {
    const input = "```math\n```";
    expect(normalizeMathCodeFences(input)).toBe(input);
  });

  it("preserves surrounding text and unrelated fences", () => {
    const input = "```js\nconst a = 1;\n```\n\n```math\nx^2\n```\n\nend";
    expect(normalizeMathCodeFences(input)).toBe(
      "```js\nconst a = 1;\n```\n\n$$\nx^2\n$$\n\nend",
    );
  });

  it("keeps fence indentation on the emitted $$ delimiters", () => {
    const input = "  ```math\n  x^2\n  ```";
    expect(normalizeMathCodeFences(input)).toBe("  $$\n  x^2\n  $$");
  });
});

describe("pipeline order: fence rewrite then delimiter rewrite", () => {
  it("still rewrites \\(…\\) outside converted math blocks", () => {
    const input = normalizeMathCodeFences(
      "text \\(x^2\\) end\n\n```math\n\\frac{1}{2}\n```",
    );
    expect(normalizeLatexMathDelimiters(input)).toBe(
      "text $x^2$ end\n\n$$\n\\frac{1}{2}\n$$",
    );
  });

  it("does not double-process $$ delimiters emitted by the fence rewrite", () => {
    const input = normalizeMathCodeFences("```math\na + b\n```");
    expect(normalizeLatexMathDelimiters(input)).toBe("$$\na + b\n$$");
  });
});

describe("normalizeSingleLineMathBlocks", () => {
  it("expands single-line $$…$$ to multi-line display math", () => {
    expect(normalizeSingleLineMathBlocks("before\n$$x^2$$\nafter")).toBe("before\n$$\nx^2\n$$\nafter");
  });

  it("keeps fence indentation on expansion", () => {
    expect(normalizeSingleLineMathBlocks("  $$a+b$$")).toBe("  $$\n  a+b\n  $$");
  });

  it("does not touch $$ inside code fences", () => {
    const input = "```\n$$not math$$\n```";
    expect(normalizeSingleLineMathBlocks(input)).toBe(input);
  });

  it("leaves already multi-line $$ blocks untouched", () => {
    const input = "$$\nx^2\n$$";
    expect(normalizeSingleLineMathBlocks(input)).toBe(input);
  });
});

describe("needsMathNormalize", () => {
  it("returns false for plain prose (streaming hot path)", () => {
    expect(needsMathNormalize("一段普通的中文回复，没有数学也没有代码。")).toBe(false);
  });

  it("returns true when a dollar sign is present", () => {
    expect(needsMathNormalize("inline $x^2$ math")).toBe(true);
  });

  it("returns true for LaTeX \\( \\[ delimiters", () => {
    expect(needsMathNormalize("公式 \\(x^2\\) 结束")).toBe(true);
    expect(needsMathNormalize("公式 \\[x^2\\] 结束")).toBe(true);
  });

  it("returns true when a code fence is present (math-language fences may be rewritten)", () => {
    expect(needsMathNormalize("```math\nx^2\n```")).toBe(true);
    expect(needsMathNormalize("~~~js\nconst a = 1;\n~~~")).toBe(true);
  });
});

describe("normalizeMarkdownMath（完整管线 + 前置短路）", () => {
  it("无数学信号时原样返回（同一字符串引用，零分配）", () => {
    const input = "普通回复文本。\n\n第二段。";
    expect(normalizeMarkdownMath(input)).toBe(input);
  });

  it("有信号时跑完整链：围栏改写 → 定界符改写 → 单行 $$ 展开", () => {
    expect(normalizeMarkdownMath("$$x^2$$")).toBe("$$\nx^2\n$$");
    expect(normalizeMarkdownMath("公式 \\(x^2\\)")).toBe("公式 $x^2$");
    expect(normalizeMarkdownMath("```math\nx^2\n```")).toBe("$$\nx^2\n$$");
  });
});
