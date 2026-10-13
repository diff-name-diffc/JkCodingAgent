import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { observePreviewVisibility } from "./usePreviewVisibility";

class TestDocument extends EventTarget {
  visibilityState: DocumentVisibilityState = "visible";

  setVisibility(next: DocumentVisibilityState) {
    this.visibilityState = next;
    this.dispatchEvent(new Event("visibilitychange"));
  }
}

class TestIntersectionObserver {
  static instances: TestIntersectionObserver[] = [];
  observe = vi.fn();
  disconnect = vi.fn();

  constructor(private callback: IntersectionObserverCallback) {
    TestIntersectionObserver.instances.push(this);
  }

  emit(element: HTMLElement, isIntersecting: boolean, intersectionRatio = 1) {
    const rect: DOMRectReadOnly = {
      x: 0,
      y: 0,
      width: 1,
      height: 1,
      top: 0,
      right: 1,
      bottom: 1,
      left: 0,
      toJSON: () => ({}),
    };
    this.callback(
      [
        {
          target: element,
          isIntersecting,
          intersectionRatio,
          boundingClientRect: rect,
          intersectionRect: rect,
          rootBounds: null,
          time: 0,
        },
      ],
      this as unknown as IntersectionObserver,
    );
  }
}

let testDocument: TestDocument;
const element = {} as HTMLElement;

beforeEach(() => {
  testDocument = new TestDocument();
  TestIntersectionObserver.instances = [];
  vi.stubGlobal("document", testDocument);
  vi.stubGlobal("IntersectionObserver", TestIntersectionObserver);
});

afterEach(() => vi.unstubAllGlobals());

describe("浏览器监控可见性", () => {
  it("文档可见且目标有正面积交集时才允许观察画面", () => {
    const visible = vi.fn();
    const stop = observePreviewVisibility(element, visible);
    const observer = TestIntersectionObserver.instances[0];
    expect(visible).toHaveBeenLastCalledWith(false);
    expect(observer.observe).toHaveBeenCalledWith(element);
    observer.emit(element, true, 0);
    expect(visible).toHaveBeenLastCalledWith(false);
    observer.emit(element, false, 1);
    expect(visible).toHaveBeenLastCalledWith(false);
    observer.emit(element, true, 0.5);
    expect(visible).toHaveBeenLastCalledWith(true);
    observer.emit(element, false, 0);
    expect(visible).toHaveBeenLastCalledWith(false);
    stop();
  });

  it("隐藏文档立即暂停并断开观察，恢复后等待新的交集结果", () => {
    const visible = vi.fn();
    const stop = observePreviewVisibility(element, visible);
    const observer = TestIntersectionObserver.instances[0];
    observer.emit(element, true);
    testDocument.setVisibility("hidden");
    expect(visible).toHaveBeenLastCalledWith(false);
    expect(observer.disconnect).toHaveBeenCalledOnce();
    const hiddenUpdates = visible.mock.calls.length;
    observer.emit(element, true);
    expect(visible).toHaveBeenCalledTimes(hiddenUpdates);
    testDocument.setVisibility("visible");
    const resumed = TestIntersectionObserver.instances[1];
    expect(resumed.observe).toHaveBeenCalledExactlyOnceWith(element);
    expect(visible).toHaveBeenLastCalledWith(false);
    observer.emit(element, true);
    expect(visible).toHaveBeenLastCalledWith(false);
    resumed.emit(element, true);
    expect(visible).toHaveBeenLastCalledWith(true);
    stop();
  });

  it("初次挂载在后台时不观察，切到前台后再开始", () => {
    testDocument.visibilityState = "hidden";
    const visible = vi.fn();
    const stop = observePreviewVisibility(element, visible);
    expect(TestIntersectionObserver.instances).toHaveLength(0);
    expect(visible).toHaveBeenLastCalledWith(false);
    testDocument.setVisibility("visible");
    const observer = TestIntersectionObserver.instances[0];
    expect(observer.observe).toHaveBeenCalledExactlyOnceWith(element);
    expect(visible).toHaveBeenLastCalledWith(false);
    stop();
  });

  it("卸载移除监听和观察，忽略排队回调且不会重新订阅", () => {
    const visible = vi.fn();
    const remove = vi.spyOn(testDocument, "removeEventListener");
    const stop = observePreviewVisibility(element, visible);
    const observer = TestIntersectionObserver.instances[0];
    stop();
    expect(observer.disconnect).toHaveBeenCalledOnce();
    expect(remove).toHaveBeenCalledWith("visibilitychange", expect.any(Function));
    visible.mockClear();
    observer.emit(element, true);
    testDocument.setVisibility("hidden");
    testDocument.setVisibility("visible");
    expect(visible).not.toHaveBeenCalled();
    expect(observer.observe).toHaveBeenCalledOnce();
  });
});
