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
