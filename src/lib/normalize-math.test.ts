import { describe, expect, it } from "vitest";
import { normalizeLatexMathDelimiters, normalizeMathCodeFences } from "./normalize-math";

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
