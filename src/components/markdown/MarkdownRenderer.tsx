import { lazy, memo, Suspense } from "react";
import type { MarkdownRendererProps } from "./MarkdownRendererImpl";

const MarkdownRendererImpl = lazy(() =>
  import("./MarkdownRendererImpl").then((m) => ({ default: m.MarkdownRenderer })),
);

/**
 * 遗留 react-markdown 管线的懒加载外壳（文件查看器 / 子智能体结果 /
 * Python 运行记录使用）。实现体在 ./MarkdownRendererImpl.tsx，连同
 * react-markdown / remark / rehype / katex 依赖一起按需加载，不再经静态
 * 链进入入口 chunk。fallback 先出纯文本，与实现内的大文本降级行为一致。
 * 聊天气泡不走这里——见 components/chat/markdown-renderer.tsx（streamdown）。
 */
export const MarkdownRenderer = memo(function MarkdownRenderer(props: MarkdownRendererProps) {
  return (
    <Suspense
      fallback={
        <div className={`markdown-surface markdown-surface--${props.variant ?? "chat"}`}>
          <pre style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>{props.content}</pre>
        </div>
      }
    >
      <MarkdownRendererImpl {...props} />
    </Suspense>
  );
});
