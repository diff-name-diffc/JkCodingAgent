import { useCallback, useRef } from "react";
import { isImeComposing } from "../utils";

export function useComposedInput(onKeyDown?: (e: React.KeyboardEvent) => void) {
  const composingRef = useRef(false);

  const onCompositionStart = useCallback(() => {
    composingRef.current = true;
  }, []);

  const onCompositionEnd = useCallback(() => {
    composingRef.current = false;
  }, []);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (composingRef.current || isImeComposing(e)) {
        e.preventDefault();
        return;
      }
      onKeyDown?.(e);
    },
    [onKeyDown],
  );

  return { handleKeyDown, onCompositionStart, onCompositionEnd };
}
