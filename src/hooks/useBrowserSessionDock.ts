import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BrowserStatus } from "../types";
import type { DockedBrowser } from "../components/BrowserDock";

interface BrowserSessionDockOptions {
  activeSessionId: string | null;
  projectPath: string | null;
  onOpen: () => void;
  onMinimized: () => void;
  onRestoreSession: (sessionId: string) => void;
  enabled?: boolean;
}

function isDockedState(state: string): boolean {
  return state === "minimized" || state === "page_closed";
}

export function reduceDockedBrowsers(
  previous: Map<string, DockedBrowser>,
  status: BrowserStatus,
): Map<string, DockedBrowser> {
  const { sessionId, state, url } = status;
  if (isDockedState(state)) {
    const next = new Map(previous);
    next.set(sessionId, { sessionId, state, url: url ?? null });
    return next;
  }
  if (!previous.has(sessionId)) return previous;
  const next = new Map(previous);
  next.delete(sessionId);
  return next;
}

async function runBrowserCommand(command: string, args: Record<string, unknown>): Promise<boolean> {
  try {
    await invoke(command, args);
    return true;
  } catch (error) {
    console.error(`${command} 执行失败:`, error);
    return false;
  }
}

export function useBrowserSessionDock({
  activeSessionId,
  projectPath,
  onOpen,
  onMinimized,
  onRestoreSession,
  enabled = true,
}: BrowserSessionDockOptions) {
  const [dockedBrowsers, setDockedBrowsers] = useState<Map<string, DockedBrowser>>(new Map());
  const activeSessionIdRef = useRef(activeSessionId);
  activeSessionIdRef.current = activeSessionId;
  const callbacksRef = useRef({ enabled, onOpen, onMinimized });
  callbacksRef.current = { enabled, onOpen, onMinimized };

  useEffect(() => {
    const unlisten = listen<BrowserStatus>("browser-status", (event) => {
      const { sessionId, state } = event.payload;

      setDockedBrowsers((previous) => reduceDockedBrowsers(previous, event.payload));

      if (sessionId !== activeSessionIdRef.current) return;
      const callbacks = callbacksRef.current;
      if (!callbacks.enabled) return;
      if (isDockedState(state)) {
        callbacks.onMinimized();
      } else if (state !== "closed") {
        callbacks.onOpen();
      }
    });

    return () => {
      unlisten.then((dispose) => dispose()).catch(console.error);
    };
  }, []);

  const minimize = useCallback(async () => {
    if (!activeSessionId) return;
    await runBrowserCommand("browser_minimize", { sessionId: activeSessionId });
  }, [activeSessionId]);

  const restore = useCallback(
    async (sessionId: string) => {
      if (!(await runBrowserCommand("browser_restore", { sessionId }))) return;
      onRestoreSession(sessionId);
      onOpen();
    },
    [onOpen, onRestoreSession],
  );

  const closeDocked = useCallback(async (sessionId: string) => {
    if (!(await runBrowserCommand("browser_stop", { sessionId }))) return;
    setDockedBrowsers((previous) => {
      if (!previous.has(sessionId)) return previous;
      const next = new Map(previous);
      next.delete(sessionId);
      return next;
    });
  }, []);

  const reopen = useCallback(async () => {
    if (!activeSessionId) return;
    if (!(await runBrowserCommand("browser_reopen", { sessionId: activeSessionId }))) return;
    onOpen();
  }, [activeSessionId, onOpen]);

  const openUrl = useCallback(
    async (url: string) => {
      if (!activeSessionId) return;
      const succeeded = await runBrowserCommand("browser_navigate", {
        sessionId: activeSessionId,
        url,
        projectPath,
      });
      if (succeeded) onOpen();
    },
    [activeSessionId, onOpen, projectPath],
  );

  const dockedSessions = useMemo(() => Array.from(dockedBrowsers.values()), [dockedBrowsers]);

  return { dockedSessions, minimize, restore, closeDocked, reopen, openUrl };
}
