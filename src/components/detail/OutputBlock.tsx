import { cn } from "../../lib/cn";

export interface OutputBlockProps {
  text: string;
  /** 空内容时的可见占位（区分「无输出」与渲染缺失）。 */
  emptyHint?: string;
  /** 超长截断上限；保留尾部最新内容（报错与结论多在结尾）。缺省全文渲染，滚动由容器承担。 */
  maxChars?: number;
  tone?: "default" | "error";
  className?: string;
}

/**
 * 详情输出块（UI-14）：stdout / Agent 输出 / 最终结果 / 产物全文的统一
 * 渲染——等宽 pre + 细滚动条 + 可选尾部截断，错误态红色边框。
 */
export function OutputBlock({
  text,
  emptyHint = "暂无内容",
  maxChars,
  tone = "default",
  className,
}: OutputBlockProps) {
  if (!text || !text.trim()) {
    return <div className={cn("ai-output-block-empty", className)}>{emptyHint}</div>;
  }
  const display = maxChars != null && text.length > maxChars ? text.slice(-maxChars) : text;
  const omitted = text.length - display.length;
  return (
    <pre
      className={cn(
        "ai-output-block chat-scroll",
        tone === "error" && "ai-output-block--error",
        className,
      )}
    >
      {omitted > 0 ? `…（前 ${omitted.toLocaleString()} 字符已省略）\n` : ""}
      {display}
    </pre>
  );
}
