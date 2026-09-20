/**
 * 架构画布场景的本地持久化（Excalidraw 无内置 persistenceKey，自建）。
 *
 * 存 localStorage（单文档，键沿用 `jkcodingagent.*` 惯例）；onChange 防抖
 * 落盘，挂载时经 restoreElements 修复后作为 initialData 恢复。
 * 后续做多文档/后端存储时换掉 STORAGE_KEY 读取处即可。
 */

import { restoreElements } from "@excalidraw/excalidraw";
import type { ExcalidrawElement } from "@excalidraw/excalidraw/element/types";

export const ARCHITECTURE_SCENE_STORAGE_KEY = "jkcodingagent.architecture.scene.v1";

const SAVE_DEBOUNCE_MS = 800;

/** 读取持久化场景；无数据或数据损坏时返回空数组（损坏不阻断画布启动）。 */
export function loadCanvasScene(): ExcalidrawElement[] {
  try {
    const raw = localStorage.getItem(ARCHITECTURE_SCENE_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as { elements?: unknown };
    if (!Array.isArray(parsed.elements)) return [];
    // restoreElements 修复缺字段/悬挂绑定，兼容旧版本写入的数据。
    return restoreElements(parsed.elements as ExcalidrawElement[], null) as ExcalidrawElement[];
  } catch (error) {
    console.error("架构画布场景恢复失败，按空画布启动:", error);
    return [];
  }
}

/** 防抖落盘器：返回触发函数（高频 onChange 直接喂给它）。 */
export function createScenePersister(): (elements: readonly ExcalidrawElement[]) => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  return (elements) => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      try {
        localStorage.setItem(ARCHITECTURE_SCENE_STORAGE_KEY, JSON.stringify({ elements }));
      } catch (error) {
        // 容量超限等写入失败只告警——画布内存态不受影响，下次变更再试。
        console.error("架构画布场景落盘失败:", error);
      }
    }, SAVE_DEBOUNCE_MS);
  };
}
