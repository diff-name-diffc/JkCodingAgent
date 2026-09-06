import type { ReactNode } from "react";
import type { DispatcherToolArtifact } from "../../types";
import { OutputBlock } from "../detail/OutputBlock";

/**
 * 产物内容按 kind 分派的注册表（UI-20）。后端当前唯一写入 kind 为
 * tool_raw_output（纯文本，agent/db/artifacts.rs）；未来新增 kind（图像/
 * Markdown 等）在此注册专用渲染器即可，未命中的 kind 走文本兜底——
 * 可扩展分派 + 永不静默丢内容。
 */
export const ARTIFACT_CONTENT_RENDERERS: Record<
  string,
  (artifact: DispatcherToolArtifact) => ReactNode
> = {
  tool_raw_output: (artifact) => <ArtifactTextContent artifact={artifact} />,
};

const CONTENT_CLASS = "min-h-40 rounded-lg border border-border bg-background p-3 text-foreground";

function ArtifactTextContent({ artifact }: { artifact: DispatcherToolArtifact }) {
  return (
    <OutputBlock text={artifact.content} emptyHint="产物内容为空" className={CONTENT_CLASS} />
  );
}

export function renderArtifactContent(artifact: DispatcherToolArtifact): ReactNode {
  const renderer = ARTIFACT_CONTENT_RENDERERS[artifact.kind];
  if (renderer) return renderer(artifact);
  // 未注册 kind：文本兜底并标注 kind，不虚构专用渲染。
  return <ArtifactTextContent artifact={artifact} />;
}
