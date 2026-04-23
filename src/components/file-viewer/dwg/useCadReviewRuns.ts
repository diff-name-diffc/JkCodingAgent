import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CadReviewRun, CadReviewRunDetail } from "../../../types";

export function useCadReviewRuns({
  workspaceId,
  filePath,
  activeReviewRunId,
  activeIssueId,
  onLocateResultMessage,
  onActiveReviewRunChange,
  onActiveIssueChange,
}: {
  workspaceId: string | null;
  filePath: string;
  activeReviewRunId: string | null;
  activeIssueId: string | null;
  onLocateResultMessage?: (messageId: string | null) => void;
  onActiveReviewRunChange: (runId: string | null) => void;
  onActiveIssueChange: (issueId: string | null) => void;
}) {
  const [reviewRuns, setReviewRuns] = useState<CadReviewRun[]>([]);
  const [reviewDetail, setReviewDetail] = useState<CadReviewRunDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadReviewDetail = useCallback(
    async (runId: string) => {
      try {
        const detail = await invoke<CadReviewRunDetail>("dispatcher_get_cad_review_run_detail", {
          runId,
        });
        setReviewDetail(detail);
        setError(null);
        return detail;
      } catch (nextError) {
        setError(nextError instanceof Error ? nextError.message : String(nextError));
        return null;
      }
    },
    [],
  );

  const loadReviewRuns = useCallback(
    async (preferredRunId?: string | null, options?: { reloadActiveDetail?: boolean }) => {
      try {
        const runs = await invoke<CadReviewRun[]>("dispatcher_list_cad_review_runs", {
          filePath,
        });
        setReviewRuns(runs);
        setError(null);
        if (runs.length === 0) {
          setReviewDetail(null);
          onActiveReviewRunChange(null);
          onActiveIssueChange(null);
          onLocateResultMessage?.(null);
          return null;
        }
        const resolvedRunId =
          preferredRunId && runs.some((run) => run.id === preferredRunId)
            ? preferredRunId
            : activeReviewRunId && runs.some((run) => run.id === activeReviewRunId)
              ? activeReviewRunId
              : runs[0].id;
        if (resolvedRunId !== activeReviewRunId) {
          onActiveReviewRunChange(resolvedRunId);
        } else if (options?.reloadActiveDetail) {
          await loadReviewDetail(resolvedRunId);
        }
        return resolvedRunId;
      } catch (nextError) {
        setError(nextError instanceof Error ? nextError.message : String(nextError));
        return null;
      }
    },
    [
      activeReviewRunId,
      filePath,
      loadReviewDetail,
      onActiveIssueChange,
      onActiveReviewRunChange,
      onLocateResultMessage,
    ],
  );

  useEffect(() => {
    void loadReviewRuns();
  }, [loadReviewRuns]);

  useEffect(() => {
    if (!activeReviewRunId) {
      setReviewDetail(null);
      onLocateResultMessage?.(null);
      return;
    }
    void loadReviewDetail(activeReviewRunId);
  }, [activeReviewRunId, loadReviewDetail, onLocateResultMessage]);

  useEffect(() => {
    if (!reviewDetail) {
      return;
    }
    if (activeIssueId && !reviewDetail.issues.some((issue) => issue.id === activeIssueId)) {
      // 首次打开 DWG 时保持整图视角，只有显式选择问题时才触发定位。
      onActiveIssueChange(null);
    }
  }, [activeIssueId, onActiveIssueChange, reviewDetail]);

  useEffect(() => {
    const issue = reviewDetail?.issues.find((value) => value.id === activeIssueId) ?? null;
    onLocateResultMessage?.(issue ? (reviewDetail?.run.resultMessageId ?? null) : null);
  }, [activeIssueId, onLocateResultMessage, reviewDetail]);

  useEffect(() => {
    if (!workspaceId) {
      return;
    }
    let disposed = false;
    const unsubscribers: Array<() => void> = [];
    const handleRunSaved = (event: {
      payload: { workspaceId: string; filePath: string; runId: string };
    }) => {
      if (disposed) return;
      if (event.payload.workspaceId !== workspaceId || event.payload.filePath !== filePath) {
        return;
      }
      void loadReviewRuns(event.payload.runId, { reloadActiveDetail: true });
    };
    void listen<{ workspaceId: string; filePath: string; runId: string }>(
      "cad-review/run-created",
      handleRunSaved,
    ).then((off) => {
      unsubscribers.push(off);
    });
    void listen<{ workspaceId: string; filePath: string; runId: string }>(
      "cad-review/run-saved",
      handleRunSaved,
    ).then((off) => {
      unsubscribers.push(off);
    });
    return () => {
      disposed = true;
      for (const unsubscribe of unsubscribers) {
        unsubscribe();
      }
    };
  }, [filePath, loadReviewRuns, workspaceId]);

  return {
    reviewRuns,
    reviewDetail,
    reviewError: error,
    reloadReviewRuns: loadReviewRuns,
  };
}
