import type {
  AnyContentSegment,
  DispatcherMessage,
  ImageSegment,
  PythonCodeRunRecord,
} from "../../types";

export function getUserMessagePayload(message: DispatcherMessage): {
  text: string;
  images: ImageSegment[];
} {
  const segments = message.segments ?? [];
  const text = segments
    .filter(isTextSegment)
    .map((segment) => segment.text)
    .join("\n\n")
    .trim();
  return { text, images: segments.filter(isImageSegment) };
}

export function pythonRunKey(messageId: string, codeHash: string): string {
  return `${messageId}:${codeHash}`;
}

export function indexPythonRuns(
  records: PythonCodeRunRecord[],
): Record<string, PythonCodeRunRecord> {
  return Object.fromEntries(
    records.map((record) => [pythonRunKey(record.messageId, record.codeHash), record]),
  );
}

function isTextSegment(
  segment: AnyContentSegment,
): segment is Extract<AnyContentSegment, { type: "text" }> {
  return segment.type === "text";
}

function isImageSegment(segment: AnyContentSegment): segment is ImageSegment {
  return segment.type === "image";
}
