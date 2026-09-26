/**
 * 画布程序执行器：预解析 → 场景草稿变更 → 一次性提交 → 区域截图 → 报告。
 *
 * Excalidraw 迁移后的事务语义：全程在 SceneDraft（元素可变副本）上演算，
 * 成功才 `updateScene` 一次提交（captureUpdate IMMEDIATELY，整轮合并为一个
 * 撤销单元，用户一次 Ctrl+Z 撤销 Agent 全部操作）；任一指令失败则不提交，
 * 画布天然零副作用（等价于 tldraw 时代的 bailToMark 整体回滚）。
 */

import { invoke } from "@tauri-apps/api/core";
import { CaptureUpdateAction, exportToBlob } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import type { ExcalidrawElement } from "@excalidraw/excalidraw/element/types";
import { isDarkActive } from "../../../lib/theme";
import {
  REPARENT_PAGE_LITERAL,
  validateArchProgram,
  type ArchInstruction,
  type ArchProgram,
} from "./arch-program";
import {
  buildArchFailureReport,
  buildArchSuccessReport,
  type ArchRunStats,
} from "./arch-report";
import {
  applyResolvedInstruction,
  createApplyContext,
  type ResolvedInstruction,
} from "./arch-apply";
import { newShapeId, repairSceneBindings, type SceneDraft } from "./exc-factory";

export interface ArchExecOutcome {
  ok: boolean;
  reportText: string;
}

interface ResolveFailure {
  ok: false;
  index: number;
  type: string;
  reason: string;
}

type ResolveResult = { ok: true; resolved: ResolvedInstruction[] } | ResolveFailure;

/** 顺序解析全部引用：程序内别名（前序指令声明）或画布已有元素。 */
function resolveProgram(draft: SceneDraft, program: ArchProgram): ResolveResult {
  const aliases = new Map<string, string>();
  const deleted = new Set<string>();
  const resolvedList: ResolvedInstruction[] = [];

  const resolveTarget = (reference: string): string | null => {
    const aliased = aliases.get(reference);
    if (aliased) return deleted.has(aliased) ? null : aliased;
    if (deleted.has(reference)) return null;
    return draft.has(reference) ? reference : null;
  };

  for (let index = 0; index < program.instructions.length; index += 1) {
    const instruction = program.instructions[index];
    const fail = (reason: string): ResolveFailure => ({
      ok: false,
      index,
      type: instruction._type,
      reason,
    });

    switch (instruction._type) {
      case "create_shape": {
        const id = newShapeId();
        aliases.set(instruction.ref, id);
        let parentFrameId: string | undefined;
        if (instruction.into !== undefined) {
          const resolvedParent = resolveTarget(instruction.into);
          if (!resolvedParent) return fail(`into 引用的容器不存在：${instruction.into}`);
          parentFrameId = resolvedParent;
        }
        resolvedList.push({ instruction, createdId: id, parentFrameId });
        break;
      }
      case "create_arrow": {
        const fromId = resolveTarget(instruction.from);
        const toId = resolveTarget(instruction.to);
        if (!fromId) return fail(`from 引用的形状不存在：${instruction.from}`);
        if (!toId) return fail(`to 引用的形状不存在：${instruction.to}`);
        const id = newShapeId();
        if (instruction.ref) aliases.set(instruction.ref, id);
        resolvedList.push({ instruction, createdId: id, arrowEnds: { fromId, toId } });
        break;
      }
      case "update_shape": {
        const targetId = resolveTarget(instruction.target);
        if (!targetId) return fail(`目标形状不存在：${instruction.target}`);
        if (draft.get(targetId)?.type === "arrow") {
          return fail(`目标 ${instruction.target} 是箭头，修改箭头请用 update_arrow`);
        }
        resolvedList.push({ instruction, targetIds: [targetId] });
        break;
      }
      case "update_arrow": {
        const targetId = resolveTarget(instruction.target);
        if (!targetId) return fail(`目标箭头不存在：${instruction.target}`);
        if (draft.get(targetId)?.type !== "arrow") {
          return fail(`目标 ${instruction.target} 不是箭头，修改形状请用 update_shape`);
        }
        resolvedList.push({ instruction, targetIds: [targetId] });
        break;
      }
      case "move_shape": {
        const targetId = resolveTarget(instruction.target);
        if (!targetId) return fail(`目标形状不存在：${instruction.target}`);
        if (draft.get(targetId)?.type === "arrow") {
          return fail(
            `箭头 ${instruction.target} 的位置由两端形状决定、不能直接移动；请移动它连接的形状`,
          );
        }
        resolvedList.push({ instruction, targetIds: [targetId] });
        break;
      }
      case "delete_shape": {
        const deleteIds: string[] = [];
        for (const target of instruction.targets) {
          const targetId = resolveTarget(target);
          if (!targetId) return fail(`删除目标不存在：${target}`);
          deleteIds.push(targetId);
          // 整个解析先于执行完成：后续指令引用「本程序已删除」的形状
          //（含经别名）必须在这里直接失败，而不是执行期产生悬挂绑定。
          deleted.add(targetId);
        }
        resolvedList.push({ instruction, deleteIds });
        break;
      }
      case "layout": {
        const targetIds: string[] = [];
        for (const target of instruction.targets) {
          const targetId = resolveTarget(target);
          if (!targetId) return fail(`布局目标不存在：${target}`);
          if (draft.get(targetId)?.type === "arrow") {
            return fail(`布局目标 ${target} 是箭头，layout 只接受形状`);
          }
          targetIds.push(targetId);
        }
        resolvedList.push({ instruction, targetIds });
        break;
      }
      case "reparent": {
        const targetIds: string[] = [];
        for (const target of instruction.targets) {
          const targetId = resolveTarget(target);
          if (!targetId) return fail(`reparent 目标不存在：${target}`);
          targetIds.push(targetId);
        }
        // "page" 字面量 → 移回页面根（应用层换成 frameId=null）。
        const reparentParentId =
          instruction.parent === REPARENT_PAGE_LITERAL
            ? null
            : resolveTarget(instruction.parent);
        if (instruction.parent !== REPARENT_PAGE_LITERAL && !reparentParentId) {
          return fail(`reparent 的目标容器不存在：${instruction.parent}`);
        }
        resolvedList.push({ instruction, targetIds, reparentParentId });
        break;
      }
      case "select_shapes": {
        const targetIds: string[] = [];
        for (const target of instruction.targets) {
          const targetId = resolveTarget(target);
          if (!targetId) return fail(`选中目标不存在：${target}`);
          targetIds.push(targetId);
        }
        resolvedList.push({ instruction, targetIds });
        break;
      }
      case "camera": {
        resolvedList.push({ instruction });
        break;
      }
    }
  }
  return { ok: true, resolved: resolvedList };
}

function countInstruction(stats: ArchRunStats, instruction: ArchInstruction): void {
  switch (instruction._type) {
    case "create_shape":
      stats.created += 1;
      break;
    case "create_arrow":
      stats.arrows += 1;
      break;
    case "update_shape":
    case "update_arrow":
      stats.updated += 1;
      break;
    case "move_shape":
      stats.moved += 1;
      break;
    case "delete_shape":
      stats.deleted += 1;
      break;
    case "layout":
      stats.layouts += 1;
      break;
    case "reparent":
      stats.reparented += 1;
      break;
    case "select_shapes":
    case "camera":
      stats.views += 1;
      break;
  }
}

/** Blob → 裸 base64（去掉 data URL 前缀）。导出供感知截图复用。 */
export function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = String(reader.result ?? "");
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.onerror = () => reject(reader.error ?? new Error("read blob failed"));
    reader.readAsDataURL(blob);
  });
}

/** 区域截图导出选项（全画布感知截图与执行报告区域截图共用口径）。 */
export function canvasExportOptions(elements: ExcalidrawElement[], files: ReturnType<ExcalidrawImperativeAPI["getFiles"]>) {
  const dark = isDarkActive();
  return {
    elements,
    files,
    mimeType: "image/jpeg",
    quality: 0.8,
    maxWidthOrHeight: 1600,
    exportPadding: 24,
    appState: {
      exportWithDarkMode: dark,
      exportBackground: true,
      viewBackgroundColor: dark ? "#121212" : "#ffffff",
    },
  };
}

/** 执行后对受影响区域截图入库（失败不阻断——报告只是少了截图）。 */
async function captureAffectedRegion(
  api: ExcalidrawImperativeAPI,
  draft: SceneDraft,
  workspaceId: string,
  touchedIds: ReadonlySet<string>,
): Promise<string | null> {
  // 受影响元素 + 其绑定文本（截图要带标签才完整）
  const region: ExcalidrawElement[] = [];
  for (const el of draft.values()) {
    if (touchedIds.has(el.id) || (el.type === "text" && touchedIds.has((el as { containerId?: string }).containerId ?? ""))) {
      region.push(el);
    }
  }
  if (region.length === 0) return null;
  try {
    const blob = await exportToBlob(canvasExportOptions(region, api.getFiles()));
    const imageDataBase64 = await blobToBase64(blob);
    const saved = await invoke<{ imageId: string; mimeType: string }>("save_chat_image", {
      workspaceId,
      imageDataBase64,
      mimeType: "image/jpeg",
    });
    return saved.imageId;
  } catch (error) {
    console.error("架构画布执行区域截图失败:", error);
    return null;
  }
}

/** 执行画布程序并产出给工具的报告文本（≤950 字符）。 */
export async function runArchProgram(
  api: ExcalidrawImperativeAPI,
  workspaceId: string,
  rawProgram: unknown,
  signal?: AbortSignal,
): Promise<ArchExecOutcome> {
  const validation = validateArchProgram(rawProgram);
  if (!validation.ok) {
    return { ok: false, reportText: buildArchFailureReport(0, "program", validation.error) };
  }
  const program = validation.program;

  // 工作副本：Map 迭代序即提交层叠序；失败不提交 = 整体回滚。
  const draft: SceneDraft = new Map(
    api.getSceneElements().map((el) => [el.id, { ...el } as ExcalidrawElement]),
  );

  const resolveResult = resolveProgram(draft, program);
  if (!resolveResult.ok) {
    return {
      ok: false,
      reportText: buildArchFailureReport(resolveResult.index, resolveResult.type, resolveResult.reason),
    };
  }

  const appState = api.getAppState();
  const zoom = appState.zoom.value || 1;
  const ctx = createApplyContext(draft, {
    x: -appState.scrollX + appState.width / zoom / 2,
    y: -appState.scrollY + appState.height / zoom / 2,
  });

  const stats: ArchRunStats = {
    total: program.instructions.length,
    created: 0,
    updated: 0,
    moved: 0,
    deleted: 0,
    arrows: 0,
    layouts: 0,
    reparented: 0,
    views: 0,
  };
  const refMap = new Map<string, string>();
  const touchedIds = new Set<string>();

  let failedIndex = 0;
  let failedType = resolveResult.resolved[0]?.instruction._type ?? "apply";
  try {
    for (let index = 0; index < resolveResult.resolved.length; index += 1) {
      const resolved = resolveResult.resolved[index];
      failedIndex = index;
      failedType = resolved.instruction._type;
      applyResolvedInstruction(ctx, resolved);
      countInstruction(stats, resolved.instruction);
      if (resolved.createdId) {
        touchedIds.add(resolved.createdId);
        const instruction = resolved.instruction;
        if (instruction._type === "create_shape") refMap.set(instruction.ref, resolved.createdId);
        if (instruction._type === "create_arrow" && instruction.ref) {
          refMap.set(instruction.ref, resolved.createdId);
        }
      }
      for (const id of resolved.targetIds ?? []) touchedIds.add(id);
      for (const id of resolved.deleteIds ?? []) touchedIds.add(id);
    }
    // 程序化 updateScene 不触发编辑器绑定重算：定向修复本程序触及的
    // 绑定文本位置与箭头几何（用户手绘的无关箭头/标签原样保留）。
    repairSceneBindings(draft, new Set([...touchedIds, ...ctx.moved]));
  } catch (error) {
    const reason = error instanceof Error ? error.message : String(error);
    return {
      ok: false,
      reportText: buildArchFailureReport(failedIndex, failedType, reason),
    };
  }

  if (signal?.aborted) return { ok: false, reportText: "画布程序已取消，草稿未提交。" };
  const elements = [...draft.values()];
  api.updateScene({
    elements,
    ...(ctx.selected.size > 0
      ? { appState: { selectedElementIds: Object.fromEntries([...ctx.selected].map((id) => [id, true])) } }
      : {}),
    captureUpdate: CaptureUpdateAction.IMMEDIATELY,
  });

  // 视图指令在提交后生效（作用于真实画布）
  if (ctx.zoomSelection && ctx.selected.size > 0) {
    const selectedElements = elements.filter((el) => ctx.selected.has(el.id));
    if (selectedElements.length > 0) {
      api.scrollToContent(selectedElements, { fitToViewport: true, viewportZoomFactor: 0.9 });
    }
  }
  if (ctx.camera) {
    if (ctx.camera.mode === "fit") {
      api.scrollToContent(elements, { fitToContent: true });
    } else if (ctx.camera.point) {
      // viewport = (scene + scroll) * zoom → 居中即 scroll = size/(2*zoom) - point
      api.updateScene({
        appState: {
          scrollX: appState.width / zoom / 2 - ctx.camera.point.x,
          scrollY: appState.height / zoom / 2 - ctx.camera.point.y,
        },
      });
    }
  }

  const screenshotImageId = await captureAffectedRegion(api, draft, workspaceId, touchedIds);
  const totalShapes = elements.length;
  const successReport = buildArchSuccessReport(stats, refMap, totalShapes, screenshotImageId);
  return {
    ok: true,
    reportText: signal?.aborted ? "取消到达前画布修改已提交；" + successReport : successReport,
  };
}
