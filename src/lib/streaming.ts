/**
 * Streaming-text rendering helpers shared by the chat message components.
 *
 * The existing MarkdownRenderer already throttles streaming markdown re-parses
 * to ~7fps. These helpers cover the small UI primitives that surround the
 * markdown surface (cursor, indicator, chunk buffering for plain-text tails).
 */

/** A zero-width blinking caret used at the tail of a streaming reply. */
export function streamingCaret(): string {
  return "▋";
}

/**
 * Merge an incoming text chunk into a buffered string, returning the new
 * buffer. Trivial today, but isolated so future chunk-coalescing / rAF
 * batching can live in one place.
 */
export function appendChunk(buffer: string, chunk: string): string {
  return buffer + chunk;
}

/** Strip a trailing streaming caret if present (used when finalizing). */
export function stripCaret(text: string): string {
  return text.replace(/▋\s*$/, "");
}
