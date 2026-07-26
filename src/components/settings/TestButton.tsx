import { useRef, useState } from "react";
import { Check, ChevronDown, Loader2, Zap } from "lucide-react";

type TestState =
  | { status: "idle" }
  | { status: "testing" }
  | { status: "success"; message: string; latencyMs: number }
  | { status: "error"; message: string };

export type TestResult = Extract<TestState, { status: "success" | "error" }> | null;

/**
 * 「测试连接」按钮：idle / loading（spinner+禁用）/ 结果三态。
 * 成功在按钮旁显示绿勾 + 延迟毫秒数；失败显示红色错误摘要，点击可展开完整错误。
 * onTest 返回成功消息，抛出错误即视为失败；延迟由前端计时。
 */
export function TestButton({
  onTest,
  disabled,
  label = "测试连接",
  onResult,
}: {
  onTest: () => Promise<string>;
  disabled?: boolean;
  label?: string;
  onResult?: (result: TestResult) => void;
}) {
  const [state, setState] = useState<TestState>({ status: "idle" });
  const [expanded, setExpanded] = useState(false);
  // 防止竞态：只允许最后一次点击更新状态。
  const runRef = useRef(0);

  async function handleClick() {
    const run = ++runRef.current;
    setState({ status: "testing" });
    setExpanded(false);
    const startedAt = performance.now();
    try {
      const message = await onTest();
      if (runRef.current !== run) return;
      const result: TestState = {
        status: "success",
        message,
        latencyMs: Math.round(performance.now() - startedAt),
      };
      setState(result);
      onResult?.(result);
    } catch (error) {
      if (runRef.current !== run) return;
      const result: TestState = { status: "error", message: String(error) };
      setState(result);
      onResult?.(result);
    }
  }

  const testing = state.status === "testing";

  return (
    <div className="ai-set-test">
      <button
        type="button"
        className="ai-set-ghost-button"
        onClick={handleClick}
        disabled={disabled || testing}
      >
        {testing ? (
          <Loader2 size={16} strokeWidth={1.5} className="spin" />
        ) : (
          <Zap size={16} strokeWidth={1.5} />
        )}
        {testing ? "测试中..." : label}
      </button>
      {state.status === "success" && (
        <span className="ai-set-test-result is-success" title={state.message}>
          <Check size={16} strokeWidth={1.5} />
          {state.latencyMs}ms
        </span>
      )}
      {state.status === "error" && (
        <div className="ai-set-test-error-wrap">
          <button
            type="button"
            className="ai-set-test-result is-error"
            onClick={() => setExpanded((v) => !v)}
            title="点击展开完整错误"
          >
            <span className="ai-set-test-error-summary">{summarizeError(state.message)}</span>
            <ChevronDown
              size={14}
              strokeWidth={1.5}
              style={{
                transform: expanded ? "rotate(180deg)" : "none",
                transition: "transform var(--motion-fast) var(--motion-ease)",
              }}
            />
          </button>
          {expanded && <pre className="ai-set-test-error-detail">{state.message}</pre>}
        </div>
      )}
    </div>
  );
}

/** 错误摘要：取首行、截断，避免长 anyhow 错误链撑破布局。 */
function summarizeError(message: string): string {
  const firstLine = message.split("\n").find((line) => line.trim()) ?? message;
  return firstLine.length > 60 ? `${firstLine.slice(0, 60)}…` : firstLine;
}
