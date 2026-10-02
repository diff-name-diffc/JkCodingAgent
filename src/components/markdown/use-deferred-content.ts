import { useEffect, useRef, useState } from "react";

/** 流式段 markdown 解析节流窗口（聊天 streamdown 与遗留管线共用）。 */
export const STREAM_THROTTLE_MS = 150;

/** 超过该长度的内容首帧先渲染纯文本占位，rAF 后再切换到完整 markdown。 */
export const LARGE_TEXT_DEFER_THRESHOLD = 10_000;

/** 首帧是否降级为纯文本占位。只在挂载时求值一次（ready 后不再回退）。 */
export function shouldDeferLargeRender(contentLength: number): boolean {
  return contentLength > LARGE_TEXT_DEFER_THRESHOLD;
}

/**
 * 节流调度决策：距上次冲刷 elapsedMs 时，"immediate" 立即冲刷，否则返回
 * 定时器延迟。延迟为 throttleMs - elapsedMs（触发时刻固定在上次冲刷 +
 * throttleMs），而非重新计时——纯尾沿 debounce 在连续 token 流下永不触发。
 */
export function throttleSchedule(
  elapsedMs: number,
  throttleMs = STREAM_THROTTLE_MS,
): "immediate" | number {
  return elapsedMs >= throttleMs ? "immediate" : throttleMs - elapsedMs;
}

export interface DeferredContent {
  /** 实际参与归一化/解析的内容（流式期间为节流后的快照）。 */
  effectiveContent: string;
  /** true 时应渲染纯文本占位而非完整 markdown（仅首次挂载大文本时出现）。 */
  deferred: boolean;
}

/**
 * 聊天 markdown 渲染的输入整形（chat/markdown-renderer.tsx 与
 * markdown/MarkdownRendererImpl.tsx 共用）：
 *
 * - 流式节流：`streaming=true` 时内容最多每 150ms 冲刷一次。用「上次冲刷
 *   时间戳」调度（leading + trailing）：定时器的触发时刻固定为
 *   lastFlush + 150ms，不受 token 到达频率影响——纯尾沿 debounce 在连续
 *   token 流（~20ms/token）下定时器被反复取消、永不触发，画面会冻结到
 *   流结束。`streaming` 翻 false 时立即直通最终内容，不延迟收尾帧。
 * - 大文本首帧降级：内容超过 10KB 时首帧返回 `deferred=true`，rAF 后切换
 *   完整渲染。`ready` 一旦置位不再回退——流式增长跨越 10KB 不会闪纯文本。
 */
export function useDeferredContent(content: string, streaming: boolean): DeferredContent {
  const [throttledContent, setThrottledContent] = useState(content);
  const latestContentRef = useRef(content);
  const lastFlushRef = useRef(0);

  useEffect(() => {
    latestContentRef.current = content;
    if (!streaming) {
      lastFlushRef.current = Date.now();
      setThrottledContent(content);
      return;
    }
    const elapsed = Date.now() - lastFlushRef.current;
    const schedule = throttleSchedule(elapsed);
    if (schedule === "immediate") {
      lastFlushRef.current = Date.now();
      setThrottledContent(content);
      return;
    }
    // 窗口内：trailing edge 冲刷最新快照。触发时刻 = lastFlush + 150ms，
    // 后续 content 变化只是重排同一绝对时刻的定时器，不会推迟冲刷点。
    const id = setTimeout(() => {
      lastFlushRef.current = Date.now();
      setThrottledContent(latestContentRef.current);
    }, schedule);
    return () => clearTimeout(id);
  }, [content, streaming]);

  const effectiveContent = streaming ? throttledContent : content;

  const [ready, setReady] = useState(() => !shouldDeferLargeRender(effectiveContent.length));
  useEffect(() => {
    if (ready) return;
    const id = requestAnimationFrame(() => setReady(true));
    return () => cancelAnimationFrame(id);
  }, [ready]);

  return { effectiveContent, deferred: !ready };
}
