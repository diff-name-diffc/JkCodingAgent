import { useEffect, useMemo, useState } from "react";
import {
  getDispatcherSessionRunning,
  subscribeDispatcherSessionRunning,
} from "../components/dispatcherSessionStore";

function sameStringSet(left: Set<string>, right: Set<string>) {
  if (left.size !== right.size) return false;
  for (const item of left) {
    if (!right.has(item)) return false;
  }
  return true;
}

function updateRunningSessionSet(prev: Set<string>, sessionId: string, running: boolean) {
  if (prev.has(sessionId) === running) return prev;
  const next = new Set(prev);
  if (running) {
    next.add(sessionId);
  } else {
    next.delete(sessionId);
  }
  return next;
}

export function useDispatcherSessionRunningSet(sessionIds: string[]) {
  const [runningSessionIds, setRunningSessionIds] = useState<Set<string>>(new Set());
  const sessionIdsKey = useMemo(() => sessionIds.join("\n"), [sessionIds]);

  useEffect(() => {
    const ids = sessionIdsKey ? sessionIdsKey.split("\n") : [];
    const nextRunning = new Set(ids.filter((sessionId) => getDispatcherSessionRunning(sessionId)));
    setRunningSessionIds((prev) => (sameStringSet(prev, nextRunning) ? prev : nextRunning));

    const unsubscribers = ids.map((sessionId) =>
      subscribeDispatcherSessionRunning(sessionId, (running) => {
        setRunningSessionIds((prev) => updateRunningSessionSet(prev, sessionId, running));
      }),
    );

    return () => {
      unsubscribers.forEach((unsubscribe) => unsubscribe());
    };
  }, [sessionIdsKey]);

  return runningSessionIds;
}
