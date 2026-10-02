import type { DiagramPlugin } from "streamdown";

/**
 * mermaid 懒加载包装：@streamdown/mermaid 静态 import mermaid 核心包
 * （1MB+），而 mermaid 图在消息流中是少数。此处实现 streamdown 的
 * DiagramPlugin 结构接口，getMermaid 同步返回代理实例，首个 mermaid 代码块
 * 真正渲染时才动态 import 插件与 mermaid 本体。
 */

interface MermaidInstanceLike {
  initialize(config?: Record<string, unknown>): void;
  render(id: string, source: string): Promise<{ svg: string }>;
}

export function createLazyMermaidPlugin(): DiagramPlugin {
  return {
    name: "mermaid",
    type: "diagram",
    language: "mermaid",
    getMermaid(config) {
      let innerPromise: Promise<MermaidInstanceLike> | null = null;
      const load = () => {
        if (!innerPromise) {
          innerPromise = import("@streamdown/mermaid").then((mod) => {
            // streamdown 的 MermaidConfig 是宽松结构类型，与 mermaid 本体
            // 的严格 MermaidConfig（theme 字面量联合等）不可直接互赋。
            const getMermaid = mod.mermaid.getMermaid as (
              c?: Record<string, unknown>,
            ) => MermaidInstanceLike;
            return getMermaid(config as Record<string, unknown> | undefined);
          });
        }
        return innerPromise;
      };
      return {
        initialize() {
          // getMermaid(config) 内部已完成初始化；此处仅触发加载。
          void load();
        },
        render(id, source) {
          return load().then((inner) => inner.render(id, source));
        },
      };
    },
  };
}
