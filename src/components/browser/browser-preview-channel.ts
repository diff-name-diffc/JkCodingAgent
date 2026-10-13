import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BrowserFrameEvent, BrowserPreviewErrorEvent } from "../../types";
import { frameMatches, PreviewFrames, type PreviewScope } from "./preview-frames";
import { PreviewLease } from "./preview-lease";

const frames = new PreviewFrames();
interface Subscriber {
  scope: PreviewScope;
  onFrame: (frame: BrowserFrameEvent) => void;
  onError: (error: string | null) => void;
}
const subscribers = new Set<Subscriber>();
interface Connection {
  active: boolean;
  unlisten: Array<() => void>;
  ready: Promise<boolean>;
  error: string | null;
}
let connection: Connection | null = null;

function connect(): Connection {
  if (connection) return connection;
  const current: Connection = {
    active: true,
    unlisten: [],
    ready: Promise.resolve(false),
    error: null,
  };
  connection = current;
  const attach = async (listener: Promise<() => void>) => {
    try {
      const unlisten = await listener;
      if (current.active) current.unlisten.push(unlisten);
      else unlisten();
      return current.active;
    } catch (error) {
      current.error = `浏览器监控事件连接失败：${String(error)}`;
      return false;
    }
  };
  const frameListener = attach(
    listen<BrowserFrameEvent>("browser-frame", ({ payload }) => {
      if (!current.active) return;
      frames.receive(payload);
      for (const subscriber of subscribers) {
        if (frameMatches(subscriber.scope, payload) && frames.get(subscriber.scope) === payload) {
          subscriber.onFrame(payload);
        }
      }
    }),
  );
  const errorListener = attach(
    listen<BrowserPreviewErrorEvent>("browser-preview-error", ({ payload }) => {
      if (!current.active) return;
      for (const subscriber of subscribers) {
        if (
          subscriber.scope.sessionId === payload.sessionId &&
          (!subscriber.scope.toolCallId || subscriber.scope.toolCallId === payload.toolCallId)
        ) {
          subscriber.onError(payload.message);
        }
      }
    }),
  );
  current.ready = Promise.all([frameListener, errorListener]).then((attached) =>
    attached.every(Boolean),
  );
  return current;
}

export function observeBrowserPreview(subscriber: Subscriber): () => void {
  subscribers.add(subscriber);
  const current = connect();
  const observerId = crypto.randomUUID();
  const lease = new PreviewLease(
    (visible) =>
      invoke<void>("browser_preview_set", {
        sessionId: subscriber.scope.sessionId,
        toolCallId: subscriber.scope.toolCallId,
        observerId,
        visible,
      }),
    subscriber.onError,
  );
  // 先接好事件，再注册观察者，避免第一帧早于异步 listen() 到达。
  void current.ready.then((ready) => {
    if (!current.active || !subscribers.has(subscriber)) return;
    if (ready) {
      lease.setVisible(true);
      return;
    }
    subscriber.onError(current.error);
    // 建连失败的连接不可复用：释放已挂监听并作废缓存，后续订阅重新建连，
    // 否则常驻面板（订阅者不掉到 0）整段会话内都无法自愈。
    if (connection === current) {
      current.active = false;
      current.unlisten.forEach((unlisten) => unlisten());
      connection = null;
    }
  });
  return () => {
    subscribers.delete(subscriber);
    lease.dispose();
    if (subscribers.size === 0 && connection) {
      connection.active = false;
      connection.unlisten.forEach((unlisten) => unlisten());
      connection = null;
    }
  };
}

export const getBrowserPreviewFrame = (scope: PreviewScope) => frames.get(scope);
export const freezeBrowserPreview = (scope: PreviewScope) => frames.freeze(scope);
export const clearBrowserPreviewSession = (sessionId: string) => frames.clearSession(sessionId);
