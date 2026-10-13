import type { BrowserFrameEvent } from "../../types";

export interface PreviewScope {
  sessionId: string;
  toolCallId: string | null;
}

function scopeKey(scope: PreviewScope): string {
  return JSON.stringify([scope.sessionId, scope.toolCallId]);
}

export function frameMatches(scope: PreviewScope, frame: BrowserFrameEvent): boolean {
  return (
    scope.sessionId === frame.sessionId &&
    (scope.toolCallId === null || (scope.toolCallId === frame.toolCallId && Boolean(frame.runId)))
  );
}

export function validPreviewFrame(frame: BrowserFrameEvent): boolean {
  return (
    typeof frame.sessionId === "string" &&
    typeof frame.streamId === "string" &&
    frame.streamId.length > 0 &&
    ((frame.runId === null && frame.toolCallId === null) ||
      (typeof frame.runId === "string" &&
        frame.runId.length > 0 &&
        typeof frame.toolCallId === "string" &&
        frame.toolCallId.length > 0)) &&
    Number.isSafeInteger(frame.sequence) &&
    frame.sequence > 0 &&
    Number.isSafeInteger(frame.capturedAt) &&
    frame.capturedAt > 0 &&
    Number.isInteger(frame.width) &&
    frame.width > 0 &&
    Number.isInteger(frame.height) &&
    frame.height > 0 &&
    typeof frame.data === "string" &&
    frame.data.startsWith("data:image/jpeg;base64,")
  );
}

interface Entry {
  scope: PreviewScope;
  frame: BrowserFrameEvent | null;
  frozen: boolean;
}

/** 只缓存最近观察到的画面，供收起/重开恢复；绝不进入聊天状态或轨迹数据库。 */
export class PreviewFrames {
  private entries = new Map<string, Entry>();

  constructor(
    private readonly limit = 24,
    private readonly maxBytes = 16 * 1024 * 1024,
  ) {}

  get(scope: PreviewScope): BrowserFrameEvent | null {
    return this.entries.get(scopeKey(scope))?.frame ?? null;
  }

  freeze(scope: PreviewScope): void {
    if (!scope.toolCallId) return;
    const key = scopeKey(scope);
    const entry = this.entries.get(key) ?? { scope, frame: null, frozen: false };
    entry.frozen = true;
    this.entries.delete(key);
    this.entries.set(key, entry);
    this.trim();
  }

  receive(frame: BrowserFrameEvent): void {
    if (!validPreviewFrame(frame) || frame.data.length * 2 > this.maxBytes) return;
    this.update({ sessionId: frame.sessionId, toolCallId: null }, frame);
    if (frame.toolCallId) {
      this.update({ sessionId: frame.sessionId, toolCallId: frame.toolCallId }, frame);
    }
    this.trim();
  }

  clearSession(sessionId: string): void {
    for (const [key, entry] of this.entries) {
      if (entry.scope.sessionId === sessionId) this.entries.delete(key);
    }
  }

  private update(scope: PreviewScope, frame: BrowserFrameEvent): void {
    const key = scopeKey(scope);
    const previous = this.entries.get(key);
    if (previous?.frozen) return;
    const last = previous?.frame;
    if (last) {
      if (scope.toolCallId && frame.runId !== last.runId) return;
      if (frame.streamId === last.streamId && frame.sequence <= last.sequence) return;
      if (frame.streamId !== last.streamId && frame.capturedAt <= last.capturedAt) return;
    }
    this.entries.delete(key);
    this.entries.set(key, { scope, frame, frozen: false });
  }

  private trim(): void {
    // 同一帧可能同时被会话与执行索引引用，预算按唯一图片对象计数。
    const bytes = () =>
      [...new Set([...this.entries.values()].map((entry) => entry.frame))].reduce(
        (total, frame) => total + (frame?.data.length ?? 0) * 2,
        0,
      );
    while (this.entries.size > this.limit || bytes() > this.maxBytes) {
      const key = this.entries.keys().next().value;
      if (key === undefined) break;
      this.entries.delete(key);
    }
  }
}
