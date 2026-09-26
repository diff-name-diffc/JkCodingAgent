/**
 * 画布程序执行器（Excalidraw 版）的端到端测试：
 * 以最小假 api（getSceneElements/getAppState/updateScene/scrollToContent/getFiles）
 * 驱动 runArchProgram，验证 解析 → 草稿应用 → 一次提交 的完整链路。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

// save_chat_image 在纯前端环境不可用：区域截图失败应静默跳过（报告不含截图引用）。
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockRejectedValue(new Error("no tauri")),
}));
// exportToBlob 依赖 DOM canvas（jsdom 无 canvas 实现），stub 掉避免噪音。
vi.mock("@excalidraw/excalidraw", () => ({
  CaptureUpdateAction: { IMMEDIATELY: "IMMEDIATELY", EVENTUALLY: "EVENTUALLY", NEVER: "NEVER" },
  exportToBlob: vi.fn().mockRejectedValue(new Error("no canvas")),
}));

import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import type { ExcalidrawElement } from "@excalidraw/excalidraw/element/types";
import { runArchProgram } from "./arch-executor";

function fakeApi(initial: ExcalidrawElement[] = []) {
  let elements = [...initial];
  const updateScene = vi.fn(
    (scene: { elements?: ExcalidrawElement[]; appState?: Record<string, unknown> }) => {
      if (scene.elements) elements = [...scene.elements];
    },
  );
  const api = {
    getSceneElements: () => elements,
    getAppState: () => ({
      zoom: { value: 1 },
      scrollX: 0,
      scrollY: 0,
      width: 1600,
      height: 900,
      selectedElementIds: {},
    }),
    updateScene,
    scrollToContent: vi.fn(),
    getFiles: () => ({}),
  } as unknown as ExcalidrawImperativeAPI;
  return { api, updateScene, getElements: () => elements };
}

const base = { version: 1 } as const;

describe("runArchProgram", () => {
  beforeEach(() => vi.clearAllMocks());

  it("校验失败不触碰画布", async () => {
    const { api, updateScene } = fakeApi();
    const outcome = await runArchProgram(api, "ws", { version: 1, instructions: [{ _type: "nope" }] });
    expect(outcome.ok).toBe(false);
    expect(updateScene).not.toHaveBeenCalled();
  });

  it("创建形状 + 箭头 + 布局一次提交，报告含 ref 映射", async () => {
    const { api, updateScene, getElements } = fakeApi();
    const outcome = await runArchProgram(api, "ws", {
      ...base,
      instructions: [
        { _type: "create_shape", ref: "gw", shape: "geo", geo: "rectangle", text: "网关", x: 0, y: 0 },
        { _type: "create_shape", ref: "svc", shape: "geo", geo: "ellipse", text: "服务", x: 300, y: 0 },
        { _type: "create_arrow", from: "gw", to: "svc", label: "HTTP" },
        { _type: "layout", mode: "row", targets: ["gw", "svc"], gap: 40 },
      ],
    });
    expect(outcome.ok).toBe(true);
    expect(outcome.reportText).toContain("gw→");
    expect(updateScene).toHaveBeenCalledTimes(1);
    const elements = getElements();
    const shapes = elements.filter((el) => el.type === "rectangle" || el.type === "ellipse");
    const arrows = elements.filter((el) => el.type === "arrow");
    const texts = elements.filter((el) => el.type === "text");
    expect(shapes).toHaveLength(2);
    expect(arrows).toHaveLength(1);
    // 两个形状标签 + 一个箭头标注
    expect(texts).toHaveLength(3);
    // 双向绑定
    const gw = shapes.find((el) => el.x === 0 || el.y === 0)!;
    expect(gw.boundElements?.some((b) => b.type === "arrow")).toBe(true);
    // 箭头标注定位在中点（labelPosition 默认 0.5）
    const arrow = arrows[0] as unknown as { points: number[][]; x: number; y: number };
    const label = texts.find(
      (t) => (t as { containerId?: string }).containerId === arrows[0].id,
    )! as { x: number; width: number };
    const arrowMidX = arrow.x + arrow.points[1][0] / 2;
    expect(Math.abs(label.x + label.width / 2 - arrowMidX)).toBeLessThan(1);
  });

  it("指令失败不提交（天然回滚），画布保持原样", async () => {
    const { api, updateScene, getElements } = fakeApi();
    const outcome = await runArchProgram(api, "ws", {
      ...base,
      instructions: [
        { _type: "create_shape", ref: "a", shape: "geo", geo: "rectangle" },
        { _type: "create_shape", ref: "b", shape: "geo", geo: "rectangle", into: "a" }, // a 不是 frame
      ],
    });
    expect(outcome.ok).toBe(false);
    expect(outcome.reportText).toContain("不是 frame");
    expect(updateScene).not.toHaveBeenCalled();
    expect(getElements()).toHaveLength(0);
  });

  it("移动形状带动绑定箭头重算；删除形状级联箭头与标签", async () => {
    // 先建图
    const { api, getElements } = fakeApi();
    await runArchProgram(api, "ws", {
      ...base,
      instructions: [
        { _type: "create_shape", ref: "a", shape: "geo", geo: "rectangle", text: "A", x: 0, y: 0 },
        { _type: "create_shape", ref: "b", shape: "geo", geo: "rectangle", text: "B", x: 300, y: 0 },
        { _type: "create_arrow", from: "a", to: "b" },
      ],
    });
    const [a, b] = getElements().filter((el) => el.type === "rectangle");
    const arrowBefore = getElements().find((el) => el.type === "arrow")!;

    // 移动 b：箭头几何跟随
    const moveOutcome = await runArchProgram(api, "ws", {
      ...base,
      instructions: [{ _type: "move_shape", target: b.id, dx: 100, dy: 0 }],
    });
    expect(moveOutcome.ok).toBe(true);
    const arrowAfter = getElements().find((el) => el.type === "arrow")!;
    expect(arrowAfter.width).toBeGreaterThan(arrowBefore.width);

    // 删除 a：箭头与 a 的标签级联消失，b 保留且绑定引用被清理
    const delOutcome = await runArchProgram(api, "ws", {
      ...base,
      instructions: [{ _type: "delete_shape", targets: [a.id] }],
    });
    expect(delOutcome.ok).toBe(true);
    const remaining = getElements();
    expect(remaining.some((el) => el.type === "arrow")).toBe(false);
    expect(remaining.filter((el) => el.type === "rectangle")).toHaveLength(1);
    expect(remaining.some((el) => el.type === "text" && (el as { text?: string }).text === "A")).toBe(false);
    const bAfter = remaining.find((el) => el.id === b.id)!;
    // 只剩 b 自己的文本标签绑定，箭头绑定已清理
    expect(bAfter.boundElements?.map((be) => be.type) ?? []).toEqual(["text"]);
  });

  it("select_shapes 写入 appState 选中集合；camera fit 调用 scrollToContent", async () => {
    const { api, updateScene, } = fakeApi();
    await runArchProgram(api, "ws", {
      ...base,
      instructions: [{ _type: "create_shape", ref: "a", shape: "geo", geo: "rectangle", x: 0, y: 0 }],
    });
    const aId = getElementsId();
    function getElementsId() {
      return (api.getSceneElements() as ExcalidrawElement[])[0].id;
    }
    const outcome = await runArchProgram(api, "ws", {
      ...base,
      instructions: [
        { _type: "select_shapes", targets: [aId], zoom: true },
        { _type: "camera", mode: "fit" },
      ],
    });
    expect(outcome.ok).toBe(true);
    const calls = updateScene.mock.calls;
    const lastCall = calls[calls.length - 1][0] as {
      appState?: { selectedElementIds?: Record<string, boolean> };
    };
    expect(lastCall.appState?.selectedElementIds).toEqual({ [aId]: true });
    expect(api.scrollToContent).toHaveBeenCalled();
  });
});

it("取消的草稿不提交画布", async () => {
  const { api, updateScene } = fakeApi();
  const controller = new AbortController();
  controller.abort();
  const outcome = await runArchProgram(api, "ws", {
    version: 1,
    instructions: [{ _type: "create_shape", ref: "a", shape: "geo", geo: "rectangle", text: "A" }],
  }, controller.signal);
  expect(outcome.ok).toBe(false);
  expect(outcome.reportText).toContain("未提交");
  expect(updateScene).not.toHaveBeenCalled();
});
