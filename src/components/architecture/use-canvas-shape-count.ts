import { useEffect, useState } from "react";
import type { Editor } from "tldraw";

/**
 * 订阅画布当前页图形数量（UI-15c）。
 *
 * editor 经回调 ref 模式暴露（ ArchitectureView 不把 editor 放进渲染 state，
 * 避免高频流式渲染波及画布），因此以 editorVersion（挂载/卸载/重建计数）作为
 * 重新订阅的信号。store.listen 只监听 document scope；数量未变化时不 setState。
 */
export function useCanvasShapeCount(
  getEditor: () => Editor | null,
  editorVersion: number,
): number {
  const [shapeCount, setShapeCount] = useState(0);

  useEffect(() => {
    const editor = getEditor();
    if (!editor) {
      setShapeCount((prev) => (prev === 0 ? prev : 0));
      return;
    }
    const sync = () => {
      const count = editor.getCurrentPageShapes().length;
      setShapeCount((prev) => (prev === count ? prev : count));
    };
    sync();
    const dispose = editor.store.listen(sync, { source: "all", scope: "document" });
    return () => {
      dispose();
    };
  }, [getEditor, editorVersion]);

  return shapeCount;
}
