/**
 * 架构画板文件管理：画板统一保存到应用资源目录
 * （`~/.jkcodingagent/architecture-canvases/`，后端 `architecture_canvas` 模块），
 * 文件为标准 `.excalidraw` 场景 JSON（含 appState/files，回显完整）。
 *
 * 本模块封装 Tauri 命令调用与场景序列化/修复；旧版 localStorage 单文档
 * 场景（v1）保留读取入口，供启动时一次性迁移为画板文件。
 */

import { invoke } from "@tauri-apps/api/core";
import { restore, restoreElements, serializeAsJSON } from "@excalidraw/excalidraw";
import type { AppState, BinaryFiles } from "@excalidraw/excalidraw/types";
import type { ExcalidrawElement } from "@excalidraw/excalidraw/element/types";
import type { ArchitectureCanvasFile, ArchitectureCanvasMeta } from "../../types/architecture";

/** 可挂进 Excalidraw 的完整画布场景（initialData 三要素）。 */
export interface CanvasScene {
  elements: readonly ExcalidrawElement[];
  appState: Partial<AppState>;
  files: BinaryFiles;
}

export const EMPTY_CANVAS_SCENE: CanvasScene = { elements: [], appState: {}, files: {} };

/** 新画板默认名（与后端 `DEFAULT_CANVAS_NAME` 同值）。 */
export const DEFAULT_CANVAS_NAME = "未命名画板";

/** 当前打开画板 id 的本地记忆（跨启动回显上次画板）。 */
const LAST_CANVAS_KEY = "jkcodingagent.architecture.canvas.last.v1";
/** 旧版 localStorage 单文档场景键（仅迁移用，勿再写入）。 */
const LEGACY_SCENE_STORAGE_KEY = "jkcodingagent.architecture.scene.v1";

export function listCanvasFiles(): Promise<ArchitectureCanvasMeta[]> {
  return invoke<ArchitectureCanvasMeta[]>("architecture_canvas_list");
}

/** 读取画板文件并修复为可回显场景（restore 兜底缺字段/悬挂绑定/旧版数据）。 */
export async function loadCanvasFile(
  canvasId: string,
): Promise<{ meta: ArchitectureCanvasMeta; scene: CanvasScene }> {
  const file = await invoke<ArchitectureCanvasFile>("architecture_canvas_load", { canvasId });
  return { meta: file.meta, scene: parseCanvasData(file.dataJson) };
}

/** 保存画板（canvasId 为 null 时后端新建），返回最新元数据。 */
export function saveCanvasFile(
  canvasId: string | null,
  name: string,
  scene: CanvasScene,
): Promise<ArchitectureCanvasMeta> {
  return invoke<ArchitectureCanvasMeta>("architecture_canvas_save", {
    canvasId,
    name,
    dataJson: serializeCanvasScene(scene),
  });
}

export function deleteCanvasFile(canvasId: string): Promise<boolean> {
  return invoke<boolean>("architecture_canvas_delete", { canvasId });
}

export function renameCanvasFile(canvasId: string, name: string): Promise<ArchitectureCanvasMeta> {
  return invoke<ArchitectureCanvasMeta>("architecture_canvas_rename", { canvasId, name });
}

/** 画板文件绝对路径（「在文件夹中显示」用）。 */
export function revealCanvasFilePath(canvasId: string): Promise<string> {
  return invoke<string>("architecture_canvas_reveal_path", { canvasId });
}

/**
 * 序列化为标准 `.excalidraw` 场景 JSON：内联图片 files，清理瞬态
 * appState（选中态/编辑态等），剔除已删除元素。
 */
export function serializeCanvasScene(scene: CanvasScene): string {
  return serializeAsJSON(scene.elements, scene.appState, scene.files, "local");
}

/** 解析 `.excalidraw` JSON 并经 restore 修复；损坏时按空画布打开（不阻断启动）。 */
export function parseCanvasData(dataJson: string): CanvasScene {
  try {
    const parsed = JSON.parse(dataJson) as {
      elements?: readonly ExcalidrawElement[] | null;
      appState?: Partial<AppState> | null;
      files?: BinaryFiles | null;
    };
    const restored = restore(
      {
        elements: parsed?.elements ?? [],
        appState: parsed?.appState ?? null,
        files: parsed?.files ?? undefined,
      },
      null,
      null,
    );
    return { elements: restored.elements, appState: restored.appState, files: restored.files };
  } catch (error) {
    console.error("画板文件解析失败，按空画布打开:", error);
    return EMPTY_CANVAS_SCENE;
  }
}

/** 场景是否含可见元素（已删除元素不算；空新画板不落盘建文件）。 */
export function sceneHasVisibleElements(scene: CanvasScene): boolean {
  return scene.elements.some((element) => !element.isDeleted);
}

export function readLastCanvasId(): string | null {
  try {
    return localStorage.getItem(LAST_CANVAS_KEY);
  } catch {
    return null;
  }
}

export function writeLastCanvasId(canvasId: string | null): void {
  try {
    if (canvasId) localStorage.setItem(LAST_CANVAS_KEY, canvasId);
    else localStorage.removeItem(LAST_CANVAS_KEY);
  } catch {
    // localStorage 不可用时静默——仅影响下次启动的回显选择。
  }
}

/** 旧版 localStorage 单文档场景（v1），仅用于一次性迁移到画板文件。 */
export function loadLegacySceneElements(): ExcalidrawElement[] {
  try {
    const raw = localStorage.getItem(LEGACY_SCENE_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as { elements?: unknown };
    if (!Array.isArray(parsed.elements)) return [];
    // restoreElements 修复缺字段/悬挂绑定，兼容旧版本写入的数据。
    return restoreElements(parsed.elements as ExcalidrawElement[], null) as ExcalidrawElement[];
  } catch (error) {
    console.error("旧画布场景读取失败:", error);
    return [];
  }
}

/** 迁移成功后清除旧版场景，避免下次启动重复迁移。 */
export function clearLegacyScene(): void {
  try {
    localStorage.removeItem(LEGACY_SCENE_STORAGE_KEY);
  } catch {
    // 同上：清理失败仅意味着下次启动重试迁移。
  }
}

/** invoke 错误统一转可读文案（Tauri 错误多为 string，也可能是 Error）。 */
export function canvasErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
