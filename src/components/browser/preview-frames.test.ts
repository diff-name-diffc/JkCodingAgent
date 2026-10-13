import { describe, expect, it } from "vitest";
import type { BrowserFrameEvent } from "../../types";
import { frameMatches, PreviewFrames, validPreviewFrame } from "./preview-frames";

function frame(overrides: Partial<BrowserFrameEvent> = {}): BrowserFrameEvent {
  return {
    sessionId: "session-a",
    streamId: "stream-a",
    runId: "run-a",
    toolCallId: "call-a",
    sequence: 1,
    capturedAt: 1000,
    url: "https://example.test/",
    data: "data:image/jpeg;base64,fixture",
    width: 1280,
    height: 800,
    ...overrides,
  };
}

const session = { sessionId: "session-a", toolCallId: null };
const call = { sessionId: "session-a", toolCallId: "call-a" };

describe("PreviewFrames", () => {
  it("isolates sessions and tool calls, and pins each tool call to its first run", () => {
    const cache = new PreviewFrames();
    const first = frame();
    const anotherCall = frame({ toolCallId: "call-b", runId: "run-b", sequence: 2 });
    const anotherSession = frame({ sessionId: "session-b" });
    cache.receive(first);
    cache.receive(anotherCall);
    cache.receive(anotherSession);
    expect(cache.get(call)).toBe(first);
    expect(cache.get(session)).toBe(anotherCall);
    expect(cache.get({ sessionId: "session-b", toolCallId: "call-a" })).toBe(anotherSession);
    expect(cache.get({ sessionId: "session-b", toolCallId: "call-b" })).toBeNull();
    cache.receive(frame({ runId: "unexpected-run", sequence: 3 }));
    expect(cache.get(call)).toBe(first);
    expect(frameMatches(call, anotherSession)).toBe(false);
    expect(frameMatches(call, anotherCall)).toBe(false);
    expect(frameMatches(session, anotherCall)).toBe(true);
    expect(frameMatches(call, frame({ runId: null, toolCallId: null }))).toBe(false);
  });

  it("rejects duplicate or older sequences, and delayed frames from an older process", () => {
    const cache = new PreviewFrames();
    const first = frame({ sequence: 4 });
    cache.receive(first);
    cache.receive(frame({ sequence: 3, capturedAt: 2000 }));
    cache.receive(frame({ sequence: 4, capturedAt: 2000 }));
    expect(cache.get(call)).toBe(first);
    const restarted = frame({ streamId: "stream-b", sequence: 1, capturedAt: 3000 });
    cache.receive(restarted);
    expect(cache.get(call)).toBe(restarted);
    cache.receive(frame({ sequence: 100, capturedAt: 2500 }));
    cache.receive(frame({ sequence: 100, capturedAt: 3000 }));
    expect(cache.get(session)).toBe(restarted);
    expect(cache.get(call)).toBe(restarted);
  });

  it("freezes the last execution image and also rejects a first frame arriving after completion", () => {
    const cache = new PreviewFrames();
    const first = frame();
    cache.receive(first);
    cache.freeze(call);
    const late = frame({ sequence: 2 });
    cache.receive(late);
    expect(cache.get(call)).toBe(first);
    expect(cache.get(session)).toBe(late);
    const emptyCall = { sessionId: "session-a", toolCallId: "empty-call" };
    cache.freeze(emptyCall);
    cache.receive(frame({ toolCallId: "empty-call", runId: "empty-run", sequence: 3 }));
    expect(cache.get(emptyCall)).toBeNull();
  });

  it("clears all images and frozen markers for only the requested session", () => {
    const cache = new PreviewFrames();
    cache.receive(frame());
    cache.freeze(call);
    const other = frame({ sessionId: "session-b" });
    cache.receive(other);
    cache.clearSession("session-a");
    expect(cache.get(session)).toBeNull();
    expect(cache.get(call)).toBeNull();
    expect(cache.get({ sessionId: "session-b", toolCallId: null })).toBe(other);
    const fresh = frame({ runId: "fresh-run" });
    cache.receive(fresh);
    expect(cache.get(call)).toBe(fresh);
  });

  it("bounds entry count, including frozen entries", () => {
    const cache = new PreviewFrames(2);
    cache.receive(frame());
    cache.freeze({ sessionId: "session-a", toolCallId: "call-b" });
    expect(cache.get(session)).toBeNull();
    expect(cache.get(call)).not.toBeNull();
    const next = frame({ sessionId: "session-b" });
    cache.receive(next);
    expect(cache.get(call)).toBeNull();
    expect(cache.get({ sessionId: "session-b", toolCallId: null })).toBe(next);
  });

  it("counts shared image data once and evicts older images to fit the byte budget", () => {
    const first = frame();
    const bytes = first.data.length * 2;
    const cache = new PreviewFrames(24, bytes);
    cache.receive(first);
    expect(cache.get(session)).toBe(first);
    expect(cache.get(call)).toBe(first);
    const second = frame({ sessionId: "session-b" });
    cache.receive(second);
    expect(cache.get(session)).toBeNull();
    expect(cache.get(call)).toBeNull();
    expect(cache.get({ sessionId: "session-b", toolCallId: null })).toBe(second);
    cache.receive(frame({ data: first.data + "oversized" }));
    expect(cache.get(session)).toBeNull();
    expect(cache.get({ sessionId: "session-b", toolCallId: null })).toBe(second);
  });

  it.each<Partial<BrowserFrameEvent>>([
    { streamId: "" },
    { runId: null },
    { toolCallId: null },
    { sequence: 0 },
    { sequence: 1.5 },
    { capturedAt: -1 },
    { width: 0 },
    { height: 1.2 },
    { data: "data:image/png;base64,fixture" },
  ])("rejects malformed wire frames: %o", (invalid) => {
    const cache = new PreviewFrames();
    const value = frame(invalid);
    expect(validPreviewFrame(value)).toBe(false);
    cache.receive(value);
    expect(cache.get(session)).toBeNull();
  });
});
