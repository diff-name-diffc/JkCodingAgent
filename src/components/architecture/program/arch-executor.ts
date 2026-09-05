/**
 * 画布程序执行器：预解析 → 事务应用（all-or-nothing）→ 区域截图 → 报告。
 *
 * 事务语义：`markHistoryStoppingPoint` 起点 + 成功 `squashToMark`（整轮合并为
 * 一个撤销单元，用户一次 Ctrl+Z 撤销 Agent 全部操作）/ 失败 `bailToMark`
 * （整体回滚，画布无痕）。
 */

import { invoke } from "@tauri-apps/api/core";
import { createShapeId, type Editor, type TLShapeId } from "tldraw";
import {
  REPARENT_PAGE_LITERAL,
  validateArchProgram,
  type ArchInstruction,
  type ArchProgram,
} from "./arch-program";
import {
  buildArchFailureReport,
  buildArchSuccessReport,
  truncateArchReport,
  type ArchRunStats,
} from "./arch-report";
import {
  applyResolvedInstruction,
  type AutoPlaceCursor,
  type ResolvedInstruction,
} from "./arch-apply";

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

/** 顺序解析全部引用：程序内别名（前序指令声明）或画布已有形状。 */
function resolveProgram(editor: Editor, program: ArchProgram): ResolveResult {
  const aliases = new Map<string, TLShapeId>();
  const deleted = new Set<TLShapeId>();
  const resolvedList: ResolvedInstruction[] = [];

  const resolveTarget = (reference: string): TLShapeId | null => {
    const aliased = aliases.get(reference);
    if (aliased) return deleted.has(aliased) ? null : aliased;
    const shapeId = reference as TLShapeId;
    if (deleted.has(shapeId)) return null;
    return editor.getShape(shapeId) ? shapeId : null;
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
        const id = createShapeId();
        aliases.set(instruction.ref, id);
        let parentFrameId: TLShapeId | undefined;
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
        const id = createShapeId();
        if (instruction.ref) aliases.set(instruction.ref, id);
        resolvedList.push({ instruction, createdId: id, arrowEnds: { fromId, toId } });
        break;
      }
      case "update_shape": {
        const targetId = resolveTarget(instruction.target);
        if (!targetId) return fail(`目标形状不存在：${instruction.target}`);
        if (editor.getShape(targetId)?.type === "arrow") {
          return fail(`目标 ${instruction.target} 是箭头，修改箭头请用 update_arrow`);
        }
        resolvedList.push({ instruction, targetIds: [targetId] });
        break;
      }
      case "update_arrow": {
        const targetId = resolveTarget(instruction.target);
        if (!targetId) return fail(`目标箭头不存在：${instruction.target}`);
        if (editor.getShape(targetId)?.type !== "arrow") {
          return fail(`目标 ${instruction.target} 不是箭头，修改形状请用 update_shape`);
        }
        resolvedList.push({ instruction, targetIds: [targetId] });
        break;
      }
      case "move_shape": {
        const targetId = resolveTarget(instruction.target);
        if (!targetId) return fail(`目标形状不存在：${instruction.target}`);
        if (editor.getShape(targetId)?.type === "arrow") {
          return fail(
            `箭头 ${instruction.target} 的位置由两端形状决定、不能直接移动；请移动它连接的形状`,
          );
        }
        resolvedList.push({ instruction, targetIds: [targetId] });
        break;
      }
      case "delete_shape": {
        const deleteIds: TLShapeId[] = [];
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
        const targetIds: TLShapeId[] = [];
        for (const target of instruction.targets) {
          const targetId = resolveTarget(target);
          if (!targetId) return fail(`布局目标不存在：${target}`);
          if (editor.getShape(targetId)?.type === "arrow") {
            return fail(`布局目标 ${target} 是箭头，布局只适用于形状`);
          }
          targetIds.push(targetId);
        }
        resolvedList.push({ instruction, targetIds });
        break;
      }
      case "reparent": {
        const targetIds: TLShapeId[] = [];
        for (const target of instruction.targets) {
          const targetId = resolveTarget(target);
          if (!targetId) return fail(`reparent 目标不存在：${target}`);
          targetIds.push(targetId);
        }
        // "page" 字面量 → 移回页面根（应用层换成当前页 id）。
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
        const targetIds: TLShapeId[] = [];
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

/** 执行后对受影响区域截图入库（失败不阻断——报告只是少了截图）。 */
async function captureAffectedRegion(
  editor: Editor,
  workspaceId: string,
  touchedIds: ReadonlySet<TLShapeId>,
): Promise<string | null> {
  const existing = [...touchedIds].filter((id) => editor.getShape(id));
  if (existing.length === 0) return null;
  try {
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const id of existing) {
      const bounds = editor.getShapePageBounds(id);
      if (!bounds) continue;
      minX = Math.min(minX, bounds.minX);
      minY = Math.min(minY, bounds.minY);
      maxX = Math.max(maxX, bounds.maxX);
      maxY = Math.max(maxY, bounds.maxY);
    }
    const maxDim = Math.max(maxX - minX, maxY - minY);
    const scale = maxDim > 0 ? Math.min(1, 1600 / maxDim) : 1;
    const { blob } = await editor.toImage(existing, {
      format: "jpeg",
      quality: 0.8,
      background: true,
      pixelRatio: 1,
      scale,
    });
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
  editor: Editor,
  workspaceId: string,
  rawProgram: unknown,
): Promise<ArchExecOutcome> {
  const validation = validateArchProgram(rawProgram);
  if (!validation.ok) {
    return { ok: false, reportText: buildArchFailureReport(0, "program", validation.error) };
  }
  const program = validation.program;

  const resolveResult = resolveProgram(editor, program);
  if (!resolveResult.ok) {
    return {
      ok: false,
      reportText: buildArchFailureReport(resolveResult.index, resolveResult.type, resolveResult.reason),
    };
  }

  const mark = editor.markHistoryStoppingPoint("arch-run");
  const cursor: AutoPlaceCursor = { x: 0, y: 0, placed: false, frameCounts: new Map() };
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
  const touchedIds = new Set<TLShapeId>();

  // resolvedList 与 program.instructions 一一对应（解析阶段逐条压栈）。
  let failedIndex = 0;
  let failedType = resolveResult.resolved[0]?.instruction._type ?? "apply";
  try {
    for (let index = 0; index < resolveResult.resolved.length; index += 1) {
      const resolved = resolveResult.resolved[index];
      failedIndex = index;
      failedType = resolved.instruction._type;
      applyResolvedInstruction(editor, resolved, cursor);
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
    // 成功：合并为一个撤销单元（用户一次 Ctrl+Z 撤销整轮）。
    editor.squashToMark(mark);
  } catch (error) {
    const reason = error instanceof Error ? error.message : String(error);
    // 回滚自身也可能失败（editor 状态异常）：catch 住以保留原始错误如实
    // 上报——冒泡会让监听器兜底报告谎称「已整体回滚」。
    try {
      editor.bailToMark(mark);
    } catch (rollbackError) {
      console.error("画布程序回滚失败:", rollbackError);
      return {
        ok: false,
        reportText: truncateArchReport(
          `错误：第 ${failedIndex + 1} 条指令（${failedType}）失败：${reason}；且回滚未完成，画布可能有部分修改。`,
        ),
      };
    }
    return {
      ok: false,
      reportText: buildArchFailureReport(failedIndex, failedType, reason),
    };
  }

  const screenshotImageId = await captureAffectedRegion(editor, workspaceId, touchedIds);
  const totalShapes = editor.getCurrentPageShapes().length;
  return {
    ok: true,
    reportText: buildArchSuccessReport(stats, refMap, totalShapes, screenshotImageId),
  };
}
