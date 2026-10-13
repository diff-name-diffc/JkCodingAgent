import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { attachAutoScroll } from "./auto-scroll-controller";

class ScrollElement extends EventTarget {
  scrollTop = 0;
  scrollHeight = 1_200;
  clientHeight = 400;
  parentElement: ScrollElement | null = null;
  firstElementChild: ScrollElement | null = null;
  ownerDocument = new EventTarget();
  overflowY = "visible";
  overscrollBehaviorY = "auto";
  interactive = false;

  closest(): ScrollElement | undefined {
    return this.interactive ? this : this.parentElement?.closest();
  }
}

let frames: Map<number, FrameRequestCallback>;
let onResize: () => void;
let disconnect: ReturnType<typeof vi.fn>;

beforeEach(() => {
  frames = new Map();
  let id = 0;
  vi.stubGlobal("Element", ScrollElement);
  vi.stubGlobal("getComputedStyle", (element: ScrollElement) => element);
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.set(++id, callback);
    return id;
  });
  vi.stubGlobal("cancelAnimationFrame", (frame: number) => frames.delete(frame));
  disconnect = vi.fn();
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        onResize = callback;
      }
      observe() {}
      disconnect = disconnect;
    },
  );
});

afterEach(() => vi.unstubAllGlobals());

function dispatch(element: EventTarget, type: string, fields: Record<string, unknown> = {}) {
  const event = new Event(type);
  for (const [key, value] of Object.entries(fields)) {
    Object.defineProperty(event, key, { value });
  }
  element.dispatchEvent(event);
}

function wheel(element: ScrollElement, deltaY: number, target: ScrollElement = element) {
  dispatch(element, "wheel", { deltaY, deltaX: 0, ctrlKey: false, target });
}

function flushFrames() {
  const queued = [...frames.values()];
  frames.clear();
  for (const callback of queued) callback(0);
}

function setup() {
  const element = new ScrollElement();
  element.firstElementChild = new ScrollElement();
  const controller = attachAutoScroll(element as unknown as HTMLDivElement);
  flushFrames();
  return { element, controller };
}

function scroll(element: ScrollElement, top: number) {
  element.scrollTop = top;
  dispatch(element, "scroll");
}

describe("消息列表自动跟随", () => {
  it("初次挂载到底部，内容和视口增长以同一帧跟随", () => {
    const { element, controller } = setup();
    expect(element.scrollTop).toBe(800);
    element.scrollHeight = 1_500;
    onResize();
    onResize();
    expect(frames.size).toBe(1);
    flushFrames();
    expect(element.scrollTop).toBe(1_100);
    expect(controller.isFollowing()).toBe(true);
    element.clientHeight = 300;
    onResize();
    flushFrames();
    expect(element.scrollTop).toBe(1_200);
  });

  it("上滑取消已排队帧，取消前取出的旧回调也不能把视图拉回", () => {
    const { element, controller } = setup();
    element.scrollHeight += 200;
    onResize();
    const staleFrame = [...frames.values()][0];
    wheel(element, -20);
    expect(frames.size).toBe(0);
    scroll(element, 780);
    staleFrame(0);
    onResize();
    flushFrames();
    expect(element.scrollTop).toBe(780);
    expect(controller.isFollowing()).toBe(false);
  });

  it("上滑一像素也能脱离，无位移 scroll 事件不能重新吸附", () => {
    const { element, controller } = setup();
    wheel(element, -1);
    scroll(element, 799);
    dispatch(element, "scroll");
    onResize();
    flushFrames();
    expect(controller.isFollowing()).toBe(false);
    expect(element.scrollTop).toBe(799);
  });

  it("靠近底部保持脱离，真正向下触底后恢复后续增长跟随", () => {
    const { element, controller } = setup();
    scroll(element, 500);
    scroll(element, 780);
    expect(controller.isFollowing()).toBe(false);
    scroll(element, 799);
    expect(controller.isFollowing()).toBe(true);
    element.scrollHeight += 100;
    onResize();
    flushFrames();
    expect(element.scrollTop).toBe(900);
  });

  it("拖动滚动条尚未派发 scroll 时，RAF 仍尊重向上的实际位置", () => {
    const { element, controller } = setup();
    element.scrollHeight += 100;
    onResize();
    element.scrollTop = 600;
    flushFrames();
    expect(element.scrollTop).toBe(600);
    expect(controller.isFollowing()).toBe(false);
  });

  it("虚拟行高补偿不脱离跟随，也不把已脱离的列表重新吸附", () => {
    const { element, controller } = setup();
    controller.adjustScrollPosition(() => {
      element.scrollTop = 740;
    });
    dispatch(element, "scroll");
    expect(controller.isFollowing()).toBe(true);
    onResize();
    flushFrames();
    expect(element.scrollTop).toBe(800);
    scroll(element, 500);
    controller.adjustScrollPosition(() => {
      element.scrollTop = 800;
    });
    dispatch(element, "scroll");
    expect(controller.isFollowing()).toBe(false);
  });

  it("虚拟测量补偿不能吞掉尚未派发 scroll 的用户上滑", () => {
    const { element, controller } = setup();
    element.scrollTop = 600;
    controller.adjustScrollPosition(() => {
      element.scrollTop = 640;
    });
    dispatch(element, "scroll");
    onResize();
    flushFrames();
    expect(controller.isFollowing()).toBe(false);
    expect(element.scrollTop).toBe(640);
  });

  it("内层详情滚动和 overscroll-contain 边界不打断外层跟随", () => {
    const { element, controller } = setup();
    const nested = new ScrollElement();
    nested.parentElement = element;
    nested.overflowY = "auto";
    nested.scrollTop = 100;
    wheel(element, -20, nested);
    expect(controller.isFollowing()).toBe(true);
    nested.scrollTop = 0;
    nested.overscrollBehaviorY = "contain";
    wheel(element, -20, nested);
    expect(controller.isFollowing()).toBe(true);
    nested.overscrollBehaviorY = "auto";
    wheel(element, -20, nested);
    expect(controller.isFollowing()).toBe(false);
  });

  it("水平滚动代码和双指缩放不改变垂直跟随", () => {
    const { element, controller } = setup();
    dispatch(element, "wheel", { deltaX: 30, deltaY: -2 });
    dispatch(element, "wheel", { deltaX: 0, deltaY: -40, ctrlKey: true });
    expect(controller.isFollowing()).toBe(true);
  });

  it("触屏手指向下拖动立即脱离，触底后再次向下滚动可重吸附", () => {
    const { element, controller } = setup();
    dispatch(element, "touchstart", { touches: [{ clientY: 100 }] });
    dispatch(element, "touchmove", { touches: [{ clientY: 130 }] });
    expect(controller.isFollowing()).toBe(false);
    dispatch(element, "touchmove", { touches: [{ clientY: 120 }] });
    expect(controller.isFollowing()).toBe(true);
  });

  it("键盘上翻立即脱离，交互控件的方向键不影响外层", () => {
    const { element, controller } = setup();
    const input = new ScrollElement();
    input.interactive = true;
    dispatch(element, "keydown", { key: "ArrowUp", target: input });
    expect(controller.isFollowing()).toBe(true);
    dispatch(element, "keydown", { key: "PageUp" });
    expect(controller.isFollowing()).toBe(false);
    dispatch(element, "keydown", { key: "End" });
    expect(controller.isFollowing()).toBe(true);
  });

  it("拖动滚动条期间暂停增长跟随，释放到底部时重新吸附", () => {
    const { element, controller } = setup();
    dispatch(element, "pointerdown", { pointerType: "mouse" });
    element.scrollHeight += 100;
    onResize();
    flushFrames();
    expect(element.scrollTop).toBe(800);
    scroll(element, 900);
    expect(controller.isFollowing()).toBe(false);
    dispatch(element.ownerDocument, "pointerup");
    expect(controller.isFollowing()).toBe(true);
  });

  it("内容折叠导致浏览器钳到底部，不误判为向上滚动", () => {
    const { element, controller } = setup();
    element.scrollHeight = 1_000;
    scroll(element, 600);
    expect(controller.isFollowing()).toBe(true);
    element.scrollHeight = 1_400;
    onResize();
    flushFrames();
    expect(element.scrollTop).toBe(1_000);
  });

  it("WebKit 底部回弹复位不被误判为上滑", () => {
    const { element, controller } = setup();
    scroll(element, 830);
    scroll(element, 800);
    expect(controller.isFollowing()).toBe(true);
  });

  it("虚拟测量先缩短再增高，钳低的位置不必仍贴当前底部", () => {
    const { element, controller } = setup();
    // 原最大值 800 → 浏览器被中间总高钳到 540 → scroll 到达时最大值已回到 600。
    element.scrollHeight = 1_000;
    scroll(element, 540);
    expect(controller.isFollowing()).toBe(true);
    onResize();
    flushFrames();
    expect(element.scrollTop).toBe(600);
    element.scrollHeight = 1_500;
    onResize();
    flushFrames();
    expect(element.scrollTop).toBe(1_100);
  });

  it("用户上滑和虚拟总高收缩同帧发生时，上滑意图仍优先", () => {
    const { element, controller } = setup();
    onResize();
    wheel(element, -60);
    element.scrollHeight = 1_000;
    scroll(element, 540);
    onResize();
    flushFrames();
    expect(controller.isFollowing()).toBe(false);
    expect(element.scrollTop).toBe(540);
  });

  it("发送消息的外部直接触底滚动仍能重新跟随", () => {
    const { element, controller } = setup();
    scroll(element, 300);
    element.scrollHeight = 1_300;
    scroll(element, 900);
    expect(controller.isFollowing()).toBe(true);
    element.scrollHeight = 1_500;
    onResize();
    flushFrames();
    expect(element.scrollTop).toBe(1_100);
  });

  it("卸载清理观察和排队帧，会话重新挂载回到底部", () => {
    const { element, controller } = setup();
    onResize();
    controller.destroy();
    expect(disconnect).toHaveBeenCalledOnce();
    expect(frames.size).toBe(0);
    scroll(element, 100);
    expect(controller.isFollowing()).toBe(false);
    attachAutoScroll(element as unknown as HTMLDivElement);
    flushFrames();
    expect(element.scrollTop).toBe(800);
  });
});
