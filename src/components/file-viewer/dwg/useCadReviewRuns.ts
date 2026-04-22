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

  const loadReviewRuns = useCallback(async (preferredRunId?: string | null) => {
    if (!workspaceId) {
      setReviewRuns([]);
      setReviewDetail(null);
      onLocateResultMessage?.(null);
      return;
    }
    try {
      const runs = await invoke<CadReviewRun[]>("dispatcher_list_cad_review_runs", {
        workspaceId,
        filePath,
      });
      setReviewRuns(runs);
      if (runs.length === 0) {
        setReviewDetail(null);
        onActiveReviewRunChange(null);
        onActiveIssueChange(null);
        onLocateResultMessage?.(null);
        return;
      }
      const resolvedRunId =
        preferredRunId && runs.some((run) => run.id === preferredRunId)
          ? preferredRunId
          : activeReviewRunId && runs.some((run) => run.id === activeReviewRunId)
            ? activeReviewRunId
          : runs[0].id;
      if (resolvedRunId !== activeReviewRunId) {
        onActiveReviewRunChange(resolvedRunId);
      }
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    }
  }, [
    activeReviewRunId,
    filePath,
    onActiveIssueChange,
    onActiveReviewRunChange,
    onLocateResultMessage,
    workspaceId,
  ]);

  useEffect(() => {
    void loadReviewRuns();
  }, [loadReviewRuns]);

  useEffect(() => {
    if (!workspaceId || !activeReviewRunId) {
      setReviewDetail(null);
      onLocateResultMessage?.(null);
      return;
    }
    let cancelled = false;
    invoke<CadReviewRunDetail>("dispatcher_get_cad_review_run_detail", {
      workspaceId,
      runId: activeReviewRunId,
    })
      .then((detail) => {
        if (cancelled) return;
        setReviewDetail(detail);
        if (!activeIssueId || !detail.issues.some((issue) => issue.id === activeIssueId)) {
          onActiveIssueChange(detail.issues[0]?.id ?? null);
        }
      })
      .catch((nextError) => {
        if (!cancelled) {
          setError(nextError instanceof Error ? nextError.message : String(nextError));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [activeIssueId, activeReviewRunId, onActiveIssueChange, onLocateResultMessage, workspaceId]);

  useEffect(() => {
    const issue = reviewDetail?.issues.find((value) => value.id === activeIssueId) ?? null;
    if (!issue) {
      return;
    }
    onLocateResultMessage?.(reviewDetail?.run.resultMessageId ?? null);
  }, [activeIssueId, onLocateResultMessage, reviewDetail]);

  useEffect(() => {
    if (!workspaceId) {
      return;
    }
    let disposed = false;
    let unsubscribe: (() => void) | null = null;
    void listen<{ workspaceId: string; filePath: string; runId: string }>(
      "cad-review/run-created",
      (event) => {
        if (disposed) return;
        if (event.payload.workspaceId !== workspaceId || event.payload.filePath !== filePath) {
          return;
        }
        void loadReviewRuns(event.payload.runId);
      },
    ).then((off) => {
      unsubscribe = off;
    });
    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, [filePath, loadReviewRuns, workspaceId]);

  return {
    reviewRuns,
    reviewDetail,
    reviewError: error,
    reloadReviewRuns: loadReviewRuns,
  };
}
