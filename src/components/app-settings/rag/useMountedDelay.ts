import { useCallback, useEffect, useRef } from "react";

/** 可取消延时与挂载状态；卸载时清理 timer，并唤醒等待者退出异步循环。 */
export function useMountedDelay() {
  const mountedRef = useRef(true);
  const pendingDelaysRef = useRef(new Map<number, (mounted: boolean) => void>());

  const isMounted = useCallback(() => mountedRef.current, []);
  const waitWhileMounted = useCallback((delayMs: number) => {
    return new Promise<boolean>((resolve) => {
      const timeoutId = window.setTimeout(() => {
        pendingDelaysRef.current.delete(timeoutId);
        resolve(mountedRef.current);
      }, delayMs);
      pendingDelaysRef.current.set(timeoutId, resolve);
    });
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    const pendingDelays = pendingDelaysRef.current;
    return () => {
      mountedRef.current = false;
      for (const [timeoutId, resolve] of pendingDelays) {
        window.clearTimeout(timeoutId);
        resolve(false);
      }
      pendingDelays.clear();
    };
  }, []);

  return { isMounted, waitWhileMounted };
}
