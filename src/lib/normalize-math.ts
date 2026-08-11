/**
 * remark-math only recognizes `$…$` / `$$…$$` as math delimiters. Models
 * frequently emit native LaTeX delimiters `\(...\)` / `\[...\]`, so rewrite
 * those to the dollar form before rendering. Code fences and inline code
 * spans are left untouched.
 */

interface ContentRegion {
  text: string;
  isCode: boolean;
}

/** Split content into fenced-code and normal-text regions. */
function splitCodeFenceRegions(content: string): ContentRegion[] {
  const regions: ContentRegion[] = [];
  let fence: string | null = null;
  let buffer: string[] = [];

  const flush = (isCode: boolean) => {
    if (buffer.length > 0) {
      regions.push({ text: buffer.join("\n"), isCode });
      buffer = [];
    }
  };

  for (const line of content.split("\n")) {
    const fenceMatch = /^ {0,3}(`{3,}|~{3,})/.exec(line);
    const closesFence =
      fenceMatch !== null &&
      fence !== null &&
      fenceMatch[1][0] === fence[0] &&
      fenceMatch[1].length >= fence.length;

    if (fenceMatch && !fence) {
      flush(false);
      buffer.push(line);
      fence = fenceMatch[1];
      continue;
    }
    if (closesFence) {
      buffer.push(line);
      flush(true);
      fence = null;
      continue;
    }
    buffer.push(line);
  }
  flush(fence !== null);
  return regions;
}

/** Rewrite `\(...\)` → `$…$` and `\[...\]` → `$$…$$` outside inline code spans. */
function rewriteLatexDelimiters(text: string): string {
  let out = "";
  let i = 0;

  while (i < text.length) {
    const ch = text[i];

    // Inline code span: copy through verbatim (unclosed run goes to the end).
    if (ch === "`") {
      const ticks = /^`+/.exec(text.slice(i))![0];
      const close = text.indexOf(ticks, i + ticks.length);
      const end = close === -1 ? text.length : close + ticks.length;
      out += text.slice(i, end);
      i = end;
      continue;
    }

    if (ch === "\\" && (text[i + 1] === "(" || text[i + 1] === "[")) {
      const isBlock = text[i + 1] === "[";
      const close = text.indexOf(isBlock ? "\\]" : "\\)", i + 2);
      if (close !== -1) {
        // remark-math rejects inline math whose content starts/ends with
        // whitespace, so trim the inner expression.
        const inner = text.slice(i + 2, close).trim();
        if (inner) {
          out += isBlock ? `$$${inner}$$` : `$${inner}$`;
          i = close + 2;
          continue;
        }
      }
    }

    out += ch;
    i += 1;
  }

  return out;
}

/** Convert LaTeX `\(...\)` / `\[...\]` delimiters to `$…$` / `$$…$$`. */
export function normalizeLatexMathDelimiters(content: string): string {
  return splitCodeFenceRegions(content)
    .map((region) => (region.isCode ? region.text : rewriteLatexDelimiters(region.text)))
    .join("\n");
}

/** Language tags that explicitly declare a math block. */
const MATH_LANGUAGES = new Set(["math", "latex", "tex", "katex", "formula"]);

/** Language tags that carry no meaning (empty / plain text), where content sniffing is safe. */
const NEUTRAL_LANGUAGES = new Set(["", "text", "plain", "txt"]);

/** Unicode math glyphs that effectively never appear in real code. */
const MATH_SYMBOL = /[\u2211\u220F\u222B\u2202\u221A\u221E\u2248\u2260\u2264\u2265\u2261\u21D2\u21D4\u2200\u2203\u2207\u2208\u2209\u222A\u2229\u221D\u00B1\u00D7\u00F7\u2070-\u2079\u2080-\u2089]/;

/** LaTeX commands that only make sense inside math mode. */
const MATH_LATEX_COMMAND =
  /\\(?:frac|dfrac|tfrac|binom|sum|prod|int|oint|sqrt|lim|infty|approx|equiv|neq|leq|geq|propto|cdot|cdots|ldots|vdots|ddots|to|rightarrow|Rightarrow|leftarrow|Leftarrow|mapsto|alpha|beta|gamma|delta|epsilon|varepsilon|zeta|eta|theta|vartheta|iota|kappa|lambda|mu|nu|xi|pi|rho|sigma|tau|upsilon|phi|varphi|chi|psi|omega|Gamma|Delta|Theta|Lambda|Xi|Pi|Sigma|Upsilon|Phi|Psi|Omega|partial|nabla|sin|cos|tan|ln|log|exp|begin\{(?:aligned|align|cases|matrix|pmatrix|bmatrix|vmatrix|split|gathered)\})/;

/** Subscript / superscript structures (`x_{…}`, `x^{…}`, `e^x`). */
const MATH_SUBSCRIPT = /[_^]\{[^}]+\}|\^[A-Za-z0-9]/;

/** Heuristic: at least two independent math signals. */
function looksLikeMath(code: string): boolean {
  let signals = 0;
  if (MATH_SYMBOL.test(code)) signals += 1;
  if (MATH_LATEX_COMMAND.test(code)) signals += 1;
  if (MATH_SUBSCRIPT.test(code)) signals += 1;
  return signals >= 2;
}

/**
 * Rewrite fenced code blocks that actually hold math into `$$…$$` display
 * math so the KaTeX pipeline renders them. Blocks tagged with an explicit
 * math language (`math` / `latex` / `tex` …) are always rewritten; untagged
 * or plain-text blocks only when the content carries at least two
 * independent math signals. Unterminated fences (still streaming) are left
 * untouched.
 */
export function normalizeMathCodeFences(content: string): string {
  const out: string[] = [];
  let fenceMarker: string | null = null;
  let fenceIndent = "";
  let fenceLang = "";
  let fenceLines: string[] = [];

  for (const line of content.split("\n")) {
    const fenceMatch = /^ {0,3}(`{3,}|~{3,})/.exec(line);

    if (fenceMatch && fenceMarker === null) {
      fenceMarker = fenceMatch[1];
      fenceIndent = line.slice(0, line.length - line.trimStart().length);
      fenceLang = line
        .slice(fenceMatch[0].length)
        .trim()
        .split(/\s+/)[0]
        .toLowerCase();
      fenceLines = [line];
      continue;
    }

    if (fenceMarker !== null) {
      fenceLines.push(line);
      const closes =
        fenceMatch !== null &&
        fenceMatch[1][0] === fenceMarker[0] &&
        fenceMatch[1].length >= fenceMarker.length &&
        line.slice(fenceMatch[0].length).trim() === "";
      if (closes) {
        const body = fenceLines.slice(1, -1);
        const bodyText = body.join("\n");
        const hasBody = bodyText.trim() !== "";
        const explicitMath = hasBody && MATH_LANGUAGES.has(fenceLang);
        const sniffedMath =
          hasBody &&
          NEUTRAL_LANGUAGES.has(fenceLang) &&
          body.length <= 12 &&
          looksLikeMath(bodyText);
        if (explicitMath || sniffedMath) {
          out.push(`${fenceIndent}$$`, ...body, `${fenceIndent}$$`);
        } else {
          out.push(...fenceLines);
        }
        fenceMarker = null;
        fenceLines = [];
      }
      continue;
    }

    out.push(line);
  }

  // Unterminated fence (streaming): pass through verbatim.
  out.push(...fenceLines);
  return out.join("\n");
}
