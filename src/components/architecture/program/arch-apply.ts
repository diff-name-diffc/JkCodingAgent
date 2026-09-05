/**
 * 画布程序解释器的应用层：把单条类型化指令翻译成 tldraw editor 操作。
 *
 * 不做校验（校验在 Rust 权威层 + arch-program.ts 防御层完成）；这里假定
 * 指令结构合法、所有引用已解析为真实 shapeId（解析在 arch-executor.ts）。
 */

import { toRichText, type Editor, type TLShapeId, type TLShapePartial } from "tldraw";
import type {
  ArchArrowStyleProps,
  ArchCamera,
  ArchCreateArrow,
  ArchCreateShape,
  ArchInstruction,
  ArchLayout,
  ArchMoveShape,
  ArchReparent,
  ArchSelectShapes,
  ArchStyleProps,
  ArchUpdateArrow,
  ArchUpdateShape,
} from "./arch-program";
import { layoutShapes, type LayoutItem } from "./arch-layout";

/** 单条指令的解析产物：所有目标引用都已换成真实存在的 shapeId。 */
export interface ResolvedInstruction {
  instruction: ArchInstruction;
  /** create_shape/create_arrow 登记的新形状（创建后回填）。 */
  createdId?: TLShapeId;
  /** update/move/layout/select_shapes/reparent 的目标（按指令内出现顺序）。 */
  targetIds?: TLShapeId[];
  /** delete 的目标。 */
  deleteIds?: TLShapeId[];
  /** create_arrow 的两端。 */
  arrowEnds?: { fromId: TLShapeId; toId: TLShapeId };
  /** create_shape.into 解析出的父容器。 */
  parentFrameId?: TLShapeId;
  /** reparent 的目标容器；null = 移回页面根。 */
  reparentParentId?: TLShapeId | null;
}

/** 自动放置游标：首个形状视口中心，其后依次右移；容器内按槽位级联。 */
export interface AutoPlaceCursor {
  x: number;
  y: number;
  placed: boolean;
  /** 每个 frame 已自动放置的形状数（容器内槽位级联用）。 */
  frameCounts: Map<string, number>;
}

const AUTO_PLACE_STEP_X = 240;

/** frame 内自动放置：标题栏下方起排，4 列槽位级联（页面坐标）。 */
const FRAME_PLACE_PAD_X = 24;
const FRAME_PLACE_PAD_Y = 60;
const FRAME_PLACE_STEP_X = 208;
const FRAME_PLACE_STEP_Y = 132;
const FRAME_PLACE_COLS = 4;

/** tldraw 页面 id 形如 `page:xxx`。 */
function isPageId(id: string): boolean {
  return id.startsWith("page:");
}

/**
 * 可选数值字段的安全取值：仅接受有限数字。
 * 权威载荷已保证省略字段为缺席而非 `null`（Rust 序列化层归一），
 * 这里是防御层——`null`/NaN 一律视为未给出，绝不把非数字传给 tldraw。
 */
function finite(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function styleProps(style: ArchStyleProps): Record<string, unknown> {
  const props: Record<string, unknown> = {};
  if (style.color) props.color = style.color;
  if (style.labelColor) props.labelColor = style.labelColor;
  if (style.fill) props.fill = style.fill;
  if (style.size) props.size = style.size;
  if (style.dash) props.dash = style.dash;
  if (style.font) props.font = style.font;
  return props;
}

/**
 * 箭头专用样式子集：DSL 约定箭头只开放 color/labelColor/size/dash
 *（权威层不会为箭头产出 fill/font，这里是事件载荷异常时的防御）。
 */
function arrowStyleProps(style: ArchArrowStyleProps): Record<string, unknown> {
  const props: Record<string, unknown> = {};
  if (style.color) props.color = style.color;
  if (style.labelColor) props.labelColor = style.labelColor;
  if (style.size) props.size = style.size;
  if (style.dash) props.dash = style.dash;
  return props;
}

function shapeCenter(editor: Editor, id: TLShapeId): { x: number; y: number } {
  const bounds = editor.getShapePageBounds(id);
  if (bounds) return { x: bounds.midX, y: bounds.midY };
  return { x: 0, y: 0 };
}

/**
 * DSL 的绝对坐标一律是页面坐标，而 tldraw 的 shape.x/y 位于**父容器局部
 * 坐标系**（frame 内形状需扣除容器变换，页面根形状恒等——见
 * `Editor.reparentShapes` 内部的逆变换换算）。写回前统一换算；单轴更新时
 * 另一轴取该形状当前页面坐标，避免把两套坐标系混在同一次写入里。
 */
function absolutePositionInParentSpace(
  editor: Editor,
  id: TLShapeId,
  shape: { x: number; y: number },
  x: number | undefined,
  y: number | undefined,
): { x: number; y: number } {
  // getShapePageTransform 对现存形状总会返回变换；bounds 的 point 同为
  // 页面坐标，仅作异常兜底——shape.x/y 是父容器局部坐标，不能在这里冒充页面坐标。
  const currentPage =
    editor.getShapePageTransform(id)?.point() ??
    editor.getShapePageBounds(id)?.point ??
    { x: shape.x, y: shape.y };
  return editor.getPointInParentSpace(id, {
    x: x ?? currentPage.x,
    y: y ?? currentPage.y,
  });
}

function autoPlacePosition(
  editor: Editor,
  cursor: AutoPlaceCursor,
  w: number,
  h: number,
): { x: number; y: number } {
  if (!cursor.placed) {
    const viewport = editor.getViewportPageBounds();
    cursor.placed = true;
    cursor.x = viewport.midX - w / 2;
    cursor.y = viewport.midY - h / 2;
    return { x: cursor.x, y: cursor.y };
  }
  cursor.x += AUTO_PLACE_STEP_X;
  return { x: cursor.x, y: cursor.y };
}

/** frame 内槽位自动放置（页面坐标）；同一容器内逐个级联。 */
function autoPlaceInFrame(
  editor: Editor,
  cursor: AutoPlaceCursor,
  frameId: TLShapeId,
): { x: number; y: number } {
  const bounds = editor.getShapePageBounds(frameId);
  const origin = bounds ? { x: bounds.x, y: bounds.y } : { x: 0, y: 0 };
  const count = cursor.frameCounts.get(frameId) ?? 0;
  cursor.frameCounts.set(frameId, count + 1);
  const position = {
    x: origin.x + FRAME_PLACE_PAD_X + (count % FRAME_PLACE_COLS) * FRAME_PLACE_STEP_X,
    y: origin.y + FRAME_PLACE_PAD_Y + Math.floor(count / FRAME_PLACE_COLS) * FRAME_PLACE_STEP_Y,
  };
  cursor.placed = true;
  cursor.x = position.x;
  cursor.y = position.y;
  return position;
}

function applyCreateShape(
  editor: Editor,
  resolved: ResolvedInstruction,
  instruction: ArchCreateShape,
  cursor: AutoPlaceCursor,
): void {
  const id = resolved.createdId!;
  const parentFrameId = resolved.parentFrameId;
  const w = finite(instruction.w);
  const h = finite(instruction.h);
  const x = finite(instruction.x);
  const y = finite(instruction.y);
  const position =
    x !== undefined && y !== undefined
      ? { x, y }
      : parentFrameId
        ? autoPlaceInFrame(editor, cursor, parentFrameId)
        : autoPlacePosition(
            editor,
            cursor,
            w ?? (instruction.shape === "note" ? 200 : 100),
            h ?? 100,
          );
  cursor.x = position.x;
  cursor.y = position.y;

  const richText = instruction.text ? toRichText(instruction.text) : undefined;
  let partial: TLShapePartial;
  switch (instruction.shape) {
    case "geo":
      partial = {
        id,
        type: "geo",
        x: position.x,
        y: position.y,
        props: {
          geo: instruction.geo ?? "rectangle",
          ...(w !== undefined ? { w } : {}),
          ...(h !== undefined ? { h } : {}),
          ...(richText ? { richText } : {}),
          ...(instruction.align ? { align: instruction.align } : {}),
          ...styleProps(instruction),
        },
      };
      break;
    case "note":
      partial = {
        id,
        type: "note",
        x: position.x,
        y: position.y,
        props: {
          ...(richText ? { richText } : {}),
          ...(instruction.align ? { align: instruction.align } : {}),
          ...styleProps(instruction),
        },
      };
      break;
    case "text":
      partial = {
        id,
        type: "text",
        x: position.x,
        y: position.y,
        props: {
          ...(richText ? { richText } : {}),
          ...(w !== undefined ? { w } : {}),
          ...(instruction.align ? { textAlign: instruction.align } : {}),
          ...styleProps(instruction),
        },
      };
      break;
    case "frame":
      partial = {
        id,
        type: "frame",
        x: position.x,
        y: position.y,
        props: {
          w: w ?? 400,
          h: h ?? 300,
          ...(instruction.text ? { name: instruction.text } : {}),
        },
      };
      break;
    default: {
      // 校验层之外再兜一道：穷尽性检查让新增形状类型在编译期报错。
      const exhaustive: never = instruction.shape;
      throw new Error(`未知的形状类型：${String(exhaustive)}`);
    }
  }
  editor.createShapes([partial]);
  // into：页面级创建（绝对坐标语义不变）后移入容器——
  // reparentShapes 保持页面坐标，形状视觉上落进 frame 原处。
  if (parentFrameId) {
    const parent = editor.getShape(parentFrameId);
    if (!parent || parent.type !== "frame") {
      throw new Error(`into 的目标不是 frame，无法把「${instruction.ref}」放入`);
    }
    editor.reparentShapes([id], parentFrameId);
  }
}

function applyCreateArrow(editor: Editor, resolved: ResolvedInstruction, instruction: ArchCreateArrow): void {
  const arrowId = resolved.createdId!;
  const { fromId, toId } = resolved.arrowEnds!;
  const start = shapeCenter(editor, fromId);
  const end = shapeCenter(editor, toId);
  const labelPosition = finite(instruction.labelPosition);
  editor.createShapes([
    {
      id: arrowId,
      type: "arrow",
      x: 0,
      y: 0,
      props: {
        kind: instruction.kind ?? "arc",
        start,
        end,
        ...(instruction.label ? { richText: toRichText(instruction.label) } : {}),
        ...(labelPosition !== undefined ? { labelPosition } : {}),
        ...(instruction.arrowheadStart ? { arrowheadStart: instruction.arrowheadStart } : {}),
        ...(instruction.arrowheadEnd ? { arrowheadEnd: instruction.arrowheadEnd } : {}),
        ...arrowStyleProps(instruction),
      },
    },
  ]);
  // 连接 = 每端一条 arrow binding；tldraw 自动路由并随形状移动。
  editor.createBindings([
    { type: "arrow", fromId: arrowId, toId: fromId, props: { terminal: "start" } },
    { type: "arrow", fromId: arrowId, toId: toId, props: { terminal: "end" } },
  ]);
}

function applyUpdateShape(
  editor: Editor,
  resolved: ResolvedInstruction,
  instruction: ArchUpdateShape,
): void {
  const id = resolved.targetIds![0];
  const shape = editor.getShape(id);
  if (!shape) return; // 预解析保证存在；执行期被用户删除则跳过
  const props: Record<string, unknown> = {};
  if (instruction.text !== undefined) {
    if (shape.type === "frame") props.name = instruction.text;
    else props.richText = toRichText(instruction.text);
  }
  const w = finite(instruction.w);
  const h = finite(instruction.h);
  if (w !== undefined && "w" in shape.props) props.w = w;
  if (h !== undefined && "h" in shape.props) props.h = h;
  if (instruction.align !== undefined) {
    if (shape.type === "text") props.textAlign = instruction.align;
    else props.align = instruction.align;
  }
  Object.assign(props, styleProps(instruction));
  const x = finite(instruction.x);
  const y = finite(instruction.y);
  const position =
    x !== undefined || y !== undefined
      ? absolutePositionInParentSpace(editor, id, shape, x, y)
      : undefined;
  editor.updateShapes([
    {
      id,
      type: shape.type,
      ...(position ? { x: position.x, y: position.y } : {}),
      props,
    },
  ]);
}

function applyUpdateArrow(
  editor: Editor,
  resolved: ResolvedInstruction,
  instruction: ArchUpdateArrow,
): void {
  const id = resolved.targetIds![0];
  const shape = editor.getShape(id);
  if (!shape || shape.type !== "arrow") return; // 预解析保证是箭头；防御执行期变更
  const props: Record<string, unknown> = {};
  // label 给空串 = 清除标注：toRichText("") 产出空富文本。
  if (instruction.label !== undefined) props.richText = toRichText(instruction.label);
  const labelPosition = finite(instruction.labelPosition);
  if (labelPosition !== undefined) props.labelPosition = labelPosition;
  if (instruction.kind) props.kind = instruction.kind;
  if (instruction.arrowheadStart) props.arrowheadStart = instruction.arrowheadStart;
  if (instruction.arrowheadEnd) props.arrowheadEnd = instruction.arrowheadEnd;
  Object.assign(props, arrowStyleProps(instruction));
  editor.updateShapes([{ id, type: "arrow", props }]);
}

function applyMoveShape(
  editor: Editor,
  resolved: ResolvedInstruction,
  instruction: ArchMoveShape,
): void {
  const id = resolved.targetIds![0];
  const shape = editor.getShape(id);
  if (!shape) return;
  const x = finite(instruction.x);
  const y = finite(instruction.y);
  if (x !== undefined || y !== undefined) {
    // 单轴合法：未给出的轴沿用当前页面坐标（权威校验已保证两族互斥）。
    // dx/dy 分支走 nudgeShapes，其内部已按父容器旋转换算，无需处理。
    const position = absolutePositionInParentSpace(editor, id, shape, x, y);
    editor.updateShapes([{ id, type: shape.type, x: position.x, y: position.y }]);
    return;
  }
  const dx = finite(instruction.dx) ?? 0;
  const dy = finite(instruction.dy) ?? 0;
  editor.nudgeShapes([id], { x: dx, y: dy });
}

function applyLayout(editor: Editor, resolved: ResolvedInstruction, instruction: ArchLayout): void {
  const ids = resolved.targetIds!;
  const items: LayoutItem[] = [];
  let minX = Infinity;
  let minY = Infinity;
  for (const id of ids) {
    const bounds = editor.getShapePageBounds(id);
    if (!bounds) continue;
    items.push({ id, w: bounds.w, h: bounds.h });
    minX = Math.min(minX, bounds.x);
    minY = Math.min(minY, bounds.y);
  }
  if (items.length < 2) return;
  const positions = layoutShapes(items, {
    mode: instruction.mode,
    origin: instruction.origin ?? { x: minX, y: minY },
    gap: instruction.gap,
    columns: instruction.columns,
    align: instruction.align,
  });
  const updates: TLShapePartial[] = [];
  for (const id of ids) {
    const next = positions.get(id);
    const shape = editor.getShape(id);
    if (!next || !shape) continue;
    // layoutShapes 输出页面坐标；写回时换算到各形状自己的父坐标系
    //（目标可能分属不同 frame，逐个换算保持页面位置与布局结果一致）。
    const local = editor.getPointInParentSpace(id, next);
    updates.push({ id, type: shape.type, x: local.x, y: local.y });
  }
  if (updates.length > 0) editor.updateShapes(updates);
}

/**
 * 移动形状进/出 frame：`reparentShapes` 保持页面坐标（位置不变，只变归属）。
 * 约束：箭头两端由绑定决定容器、不允许手动 reparent；frame 只能位于页面根；
 * 目标容器是某目标的子孙时拒绝（循环包含）。
 */
function applyReparent(editor: Editor, resolved: ResolvedInstruction, instruction: ArchReparent): void {
  const parentId = resolved.reparentParentId ?? editor.getCurrentPageId();
  if (!isPageId(parentId)) {
    const parent = editor.getShape(parentId);
    if (!parent || parent.type !== "frame") {
      throw new Error(`reparent 的目标容器「${instruction.parent}」不是 frame`);
    }
  }

  const targetIds = resolved.targetIds!;
  const validIds: TLShapeId[] = [];
  for (const id of targetIds) {
    const shape = editor.getShape(id);
    if (!shape) continue; // 执行期已不存在（如前序指令删除）：跳过
    if (shape.type === "arrow") {
      throw new Error("箭头不能 reparent：它的容器由两端形状决定");
    }
    if (shape.type === "frame") {
      throw new Error("frame 只能位于页面根，不能作为 reparent 目标");
    }
    validIds.push(id);
  }
  if (validIds.length === 0) return;

  // 循环包含检查：目标不得是目标容器的祖先（含目标容器自身）。
  if (!isPageId(parentId)) {
    const targetSet = new Set<string>(targetIds);
    let current = editor.getShape(parentId);
    while (current) {
      if (targetSet.has(current.id)) {
        throw new Error("reparent 的目标容器位于某个被移动形状内部，会形成循环包含");
      }
      if (isPageId(current.parentId as string)) break;
      current = editor.getShape(current.parentId as TLShapeId);
    }
  }

  editor.reparentShapes(validIds, parentId);
}

/** 选中形状（让用户看到 Agent 所指），可选缩放至选中区域。 */
function applySelectShapes(
  editor: Editor,
  resolved: ResolvedInstruction,
  instruction: ArchSelectShapes,
): void {
  const existing = resolved.targetIds!.filter((id) => editor.getShape(id));
  if (existing.length === 0) return;
  editor.setSelectedShapes(existing);
  if (instruction.zoom) editor.zoomToSelection();
}

/** 相机导航：不改画布内容，只动视口。 */
function applyCamera(editor: Editor, instruction: ArchCamera): void {
  if (instruction.mode === "fit") {
    editor.zoomToFit();
    return;
  }
  const point = instruction.point;
  if (point) editor.centerOnPoint(point);
}

/**
 * 在 editor 上应用一条已解析指令（create 类会写回 resolved.createdId）。
 * editor 抛错时异常上抛，由执行器统一回滚。
 */
export function applyResolvedInstruction(
  editor: Editor,
  resolved: ResolvedInstruction,
  cursor: AutoPlaceCursor,
): void {
  const instruction = resolved.instruction;
  switch (instruction._type) {
    case "create_shape":
      applyCreateShape(editor, resolved, instruction, cursor);
      break;
    case "create_arrow":
      applyCreateArrow(editor, resolved, instruction);
      break;
    case "update_shape":
      applyUpdateShape(editor, resolved, instruction);
      break;
    case "update_arrow":
      applyUpdateArrow(editor, resolved, instruction);
      break;
    case "move_shape":
      applyMoveShape(editor, resolved, instruction);
      break;
    case "delete_shape":
      editor.deleteShapes(resolved.deleteIds!);
      break;
    case "layout":
      applyLayout(editor, resolved, instruction);
      break;
    case "reparent":
      applyReparent(editor, resolved, instruction);
      break;
    case "select_shapes":
      applySelectShapes(editor, resolved, instruction);
      break;
    case "camera":
      applyCamera(editor, instruction);
      break;
  }
}
