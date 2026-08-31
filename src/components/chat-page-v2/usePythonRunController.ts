import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type { PythonCodeRunRecord, PythonCodeRunTarget, PythonRunEvent } from "../../types";
import { indexPythonRuns, pythonRunKey } from "./message-utils";

export function usePythonRunController(
  activeSessionId: string | null,
  currentSessionIdRef: React.RefObject<string | null>,
) {
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [target, setTarget] = useState<PythonCodeRunTarget | null>(null);
  const [records, setRecords] = useState<Record<string, PythonCodeRunRecord>>({});

  useEffect(() => {
    if (!activeSessionId) {
      setTarget(null);
      setRecords({});
      return;
    }
    let cancelled = false;
    invoke<PythonCodeRunRecord[]>("python_runner_list_results", { workspaceId: activeSessionId })
      .then((items) => {
        if (!cancelled) setRecords(indexPythonRuns(items));
      })
      .catch(console.error);
    return () => {
      cancelled = true;
    };
  }, [activeSessionId]);

  useEffect(() => {
    const unlisten = listen<PythonRunEvent>("python-run-event", ({ payload }) => {
      if (payload.workspaceId !== currentSessionIdRef.current) return;
      const record = payload.data.record;
      if (record) {
        setRecords((previous) => ({
          ...previous,
          [pythonRunKey(record.messageId, record.codeHash)]: record,
        }));
      } else if (payload.event === "output") {
        setRecords((previous) => appendRunOutput(previous, payload));
      }
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [currentSessionIdRef]);

  const run = useCallback(
    async (nextTarget: PythonCodeRunTarget) => {
      if (!activeSessionId) return;
      setTarget(nextTarget);
      setDrawerOpen(true);
      try {
        const started = await invoke<PythonCodeRunRecord>("python_runner_start", {
          workspaceId: activeSessionId,
          messageId: nextTarget.messageId,
          codeBlockIndex: nextTarget.codeBlockIndex,
          code: nextTarget.code,
        });
        setRecords((previous) => ({
          ...previous,
          [pythonRunKey(started.messageId, started.codeHash)]: started,
        }));
      } catch (error) {
        console.error("启动 Python 执行失败:", error);
      }
    },
    [activeSessionId],
  );

  const stop = useCallback(async (runId: string) => {
    try {
      await invoke("python_runner_stop", { runId });
    } catch (error) {
      console.error("停止 Python 执行失败:", error);
    }
  }, []);
  const clear = useCallback(
    async (runTarget: PythonCodeRunTarget) => {
      if (!activeSessionId) return;
      try {
        await invoke("python_runner_clear_result", {
          workspaceId: activeSessionId,
          messageId: runTarget.messageId,
          codeBlockIndex: runTarget.codeBlockIndex,
        });
        setRecords((previous) => {
          const next = { ...previous };
          delete next[pythonRunKey(runTarget.messageId, runTarget.codeHash)];
          return next;
        });
      } catch (error) {
        console.error("清空 Python 执行结果失败:", error);
      }
    },
    [activeSessionId],
  );

  const selectedRecord = target
    ? (records[pythonRunKey(target.messageId, target.codeHash)] ?? null)
    : null;
  return {
    drawerOpen,
    setDrawerOpen,
    target,
    records,
    selectedRecord,
    selectedRunning: selectedRecord?.status === "running",
    run,
    stop,
    clear,
  };
}

function appendRunOutput(
  records: Record<string, PythonCodeRunRecord>,
  payload: PythonRunEvent,
): Record<string, PythonCodeRunRecord> {
  const entry = Object.entries(records).find(([, record]) => record.runId === payload.runId);
  if (!entry) return records;
  const [key, record] = entry;
  return {
    ...records,
    [key]: {
      ...record,
      stdout: record.stdout + (payload.data.stdout ?? ""),
      stderr: record.stderr + (payload.data.stderr ?? ""),
    },
  };
}
