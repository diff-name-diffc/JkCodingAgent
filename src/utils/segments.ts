import type { AnyContentSegment, TextSegment, ImageSegment } from "../types";

export function segmentsToMarkdown(segments: AnyContentSegment[]): string {
  return segments
    .map((seg) => {
      if (seg.type === "text") {
        return (seg as TextSegment).text;
      }
      if (seg.type === "image") {
        const img = seg as ImageSegment;
        // Use the chat-image:// protocol so the path stays out of user-visible
        // text. The MarkdownImage component resolves it via Tauri at render time.
        return `![${img.alt || "image"}](chat-image://${img.imageId})`;
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
    const rawRef = match[3];

    if (beforeText) {
      segments.push({
        id: crypto.randomUUID(),
        type: "text",
        text: beforeText.trim(),
      });
    }

    // Extract imageId from chat-image:// protocol when present; fall back to a
    // newly generated id for legacy absolute-path references. The path is only
    // retained for backward compatibility with existing local files — new
    // content should reference images via the chat-image:// protocol.
    const chatImageProtocol = "chat-image://";
    const isChatImageRef = rawRef.startsWith(chatImageProtocol);
    const imageId = isChatImageRef ? rawRef.slice(chatImageProtocol.length) : crypto.randomUUID();
    const path = isChatImageRef ? "" : rawRef;

    segments.push({
      id: crypto.randomUUID(),
      type: "image",
      imageId,
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
