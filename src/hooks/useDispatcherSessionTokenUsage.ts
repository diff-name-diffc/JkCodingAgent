import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DispatcherSessionTokenUsage } from "../types";

export function useDispatcherSessionTokenUsage(sessionId: string) {
  const [entries, setEntries] = useState<DispatcherSessionTokenUsage[]>([]);
  const currentSessionIdRef = useRef(sessionId);
  currentSessionIdRef.current = sessionId;
  const loadIdRef = useRef(0);

  const refresh = useCallback(async (targetSessionId = currentSessionIdRef.current) => {
    const loadId = ++loadIdRef.current;
    try {
      const loaded = await invoke<DispatcherSessionTokenUsage[]>(
        "dispatcher_get_session_token_usage",
        {
          workspaceId: targetSessionId,
        },
      );
      if (currentSessionIdRef.current !== targetSessionId || loadIdRef.current !== loadId) {
        return;
      }
      setEntries(loaded.filter((entry) => entry.workspaceId === targetSessionId));
    } catch (error) {
      if (currentSessionIdRef.current === targetSessionId && loadIdRef.current === loadId) {
        console.error("加载会话 token 占用失败:", error);
      }
    }
  }, []);

  const reset = useCallback(() => {
    loadIdRef.current += 1;
    setEntries([]);
  }, []);

  useEffect(() => {
    reset();
    void refresh(sessionId);
  }, [refresh, reset, sessionId]);

  return {
    entries,
    refresh,
    reset,
  };
}
