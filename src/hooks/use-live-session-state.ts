import { useEffect, useRef, useState } from "react";
import { subscribeDispatcherLiveSession } from "../components/dispatcherSessionStore";
import type { DispatcherLiveSessionState } from "../components/dispatcherSessionStore";

/**
 * Bridges the module-level dispatcherSessionStore singleton into React state
 * for the refactored chat surface. Reuses the existing rAF-batched pub/sub so
 * streaming performance characteristics are unchanged.
 *
 * This is the new-stack equivalent of dispatcher-chat/useLiveSessionState.ts,
 * kept minimal: it only exposes the live state for a given session id. Send /
 * channel plumbing stays in the existing useDispatcherActions hook — do not
 * duplicate it here.
 */
export function useLiveSessionState(
  sessionId: string | null,
): DispatcherLiveSessionState | null {
  const [state, setState] = useState<DispatcherLiveSessionState | null>(null);
  const stateRef = useRef<DispatcherLiveSessionState | null>(null);
  stateRef.current = state;

  useEffect(() => {
    if (!sessionId) {
      setState(null);
      return;
    }

    // Prime with current value if the store already has one.
    const unsubscribe = subscribeDispatcherLiveSession(sessionId, (next) => {
      setState(next);
    });

    return () => {
      unsubscribe();
      setState(null);
    };
  }, [sessionId]);

  return state;
}
