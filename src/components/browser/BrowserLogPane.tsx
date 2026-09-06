export interface BrowserLogPaneProps {
  error: string | null;
  logs: string[];
}

/** 浏览器日志/错误条（UI-18 拆分）：错误红字置顶，日志保留最近 31 条。 */
export function BrowserLogPane({ error, logs }: BrowserLogPaneProps) {
  if (!error && logs.length === 0) return null;
  return (
    <div className="ai-browser-log chat-scroll">
      {error && <div className="ai-browser-log-error">{error}</div>}
      {logs.map((item, index) => (
        <div key={`${index}-${item}`}>{item}</div>
      ))}
    </div>
  );
}
