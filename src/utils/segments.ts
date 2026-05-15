import type { AnyContentSegment, TextSegment, ImageSegment } from "../types";

export function segmentsToMarkdown(segments: AnyContentSegment[]): string {
  return segments
    .map((seg) => {
      if (seg.type === "text") {
        return (seg as TextSegment).text;
      }
      if (seg.type === "image") {
        const img = seg as ImageSegment;
        return `![${img.alt || "image"}](${img.path})`;
      }
      if (seg.type === "file") {
        return "";
      }
      return "";
    })
    .join("\n");
}

export function markdownToSegments(markdown: string): AnyContentSegment[] {
  const segments: AnyContentSegment[] = [];
  const regex = /([^]*)!\[([^]*)\]\(([^)]+)\)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = regex.exec(markdown)) !== null) {
    const beforeText = match[1];
    const alt = match[2];
    const path = match[3];
    if (beforeText) {
      segments.push({
        id: crypto.randomUUID(),
        type: "text",
        text: beforeText.trim(),
      });
    }
    segments.push({
      id: crypto.randomUUID(),
      type: "image",
      imageId: crypto.randomUUID(),
      path,
      alt: alt || undefined,
      source: "user_paste",
    } as ImageSegment);
    lastIndex = regex.lastIndex;
  }

  const remaining = markdown.slice(lastIndex).trim();
  if (remaining) {
    segments.push({
      id: crypto.randomUUID(),
      type: "text",
      text: remaining,
    });
  }

  return segments;
}
