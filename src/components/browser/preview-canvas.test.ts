import { describe, expect, it, vi } from "vitest";
import type { BrowserFrameEvent } from "../../types";
import { PreviewCanvas } from "./preview-canvas";

class FakeImage {
  onload: ((event: Event) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  src = "";

  load(): void {
    this.onload?.({} as Event);
  }

  fail(): void {
    this.onerror?.({} as Event);
  }
}

function frame(sequence = 1): BrowserFrameEvent {
  return {
    sessionId: "session",
    streamId: "stream",
    runId: "run",
    toolCallId: "call",
    sequence,
    capturedAt: 1000 + sequence,
    url: null,
    data: `data:image/jpeg;base64,frame-${sequence}`,
    width: 1280 + sequence,
    height: 800 + sequence,
  };
}

function fakeCanvas(available = true) {
  const context = { drawImage: vi.fn() };
  const element = {
    width: 100,
    height: 100,
    getContext: vi.fn(() => (available ? context : null)),
  };
  return { element, context, canvas: element as unknown as HTMLCanvasElement };
}

function setup() {
  const images: FakeImage[] = [];
  const onError = vi.fn();
  const painter = new PreviewCanvas(onError, () => {
    const image = new FakeImage();
    images.push(image);
    return image as unknown as HTMLImageElement;
  });
  return { painter, images, onError, ...fakeCanvas() };
}

describe("PreviewCanvas", () => {
  it("decodes only one image at a time and merges pending frames into the latest", () => {
    const { painter, images, canvas, element, context } = setup();
    painter.present(canvas, frame(1));
    painter.present(canvas, frame(2));
    const latest = frame(3);
    painter.present(canvas, latest);
    expect(images).toHaveLength(1);
    images[0].load();
    expect(context.drawImage).not.toHaveBeenCalled();
    expect(images).toHaveLength(2);
    expect(images[1].src).toBe(latest.data);
    images[1].load();
    expect(context.drawImage).toHaveBeenCalledExactlyOnceWith(
      images[1],
      0,
      0,
      latest.width,
      latest.height,
    );
    expect(element.width).toBe(latest.width);
    expect(element.height).toBe(latest.height);
    painter.present(canvas, latest);
    expect(images).toHaveLength(2);
  });

  it("pause rejects an already queued load callback and permits a fresh image on resume", () => {
    const { painter, images, canvas, context } = setup();
    painter.present(canvas, frame());
    const lateLoad = images[0].onload;
    painter.pause();
    expect(images[0].src).toBe("");
    expect(images[0].onload).toBeNull();
    painter.present(canvas, frame(2));
    lateLoad?.({} as Event);
    expect(context.drawImage).not.toHaveBeenCalled();
    images[1].load();
    expect(context.drawImage).toHaveBeenCalledTimes(1);
  });

  it("clear erases the canvas and rejects late load and error callbacks", () => {
    const { painter, images, canvas, element, context, onError } = setup();
    painter.present(canvas, frame());
    const lateLoad = images[0].onload;
    const lateError = images[0].onerror;
    painter.clear(canvas);
    expect(element.width).toBe(0);
    expect(element.height).toBe(0);
    lateLoad?.({} as Event);
    lateError?.({} as Event);
    expect(context.drawImage).not.toHaveBeenCalled();
    expect(onError).not.toHaveBeenCalled();
  });

  it("a replacement canvas cannot receive the old canvas's in-flight image", () => {
    const { painter, images, canvas, context } = setup();
    const replacement = fakeCanvas();
    painter.present(canvas, frame());
    const lateLoad = images[0].onload;
    painter.present(replacement.canvas, frame(2));
    lateLoad?.({} as Event);
    expect(context.drawImage).not.toHaveBeenCalled();
    expect(replacement.context.drawImage).not.toHaveBeenCalled();
    images[1].load();
    expect(replacement.context.drawImage).toHaveBeenCalledTimes(1);
    expect(context.drawImage).not.toHaveBeenCalled();
  });

  it("skips stale decode failures and continues with the latest frame", () => {
    const { painter, images, canvas, context, onError } = setup();
    painter.present(canvas, frame());
    painter.present(canvas, frame(2));
    images[0].fail();
    expect(onError).not.toHaveBeenCalled();
    expect(images).toHaveLength(2);
    images[1].fail();
    expect(onError).toHaveBeenCalledExactlyOnceWith("浏览器监控画面解码失败");
    painter.present(canvas, frame(3));
    images[2].load();
    expect(context.drawImage).toHaveBeenCalledTimes(1);
  });

  it("reports a missing drawing context without retaining a blocked decoder", () => {
    const { painter, images, onError } = setup();
    const missing = fakeCanvas(false);
    painter.present(missing.canvas, frame());
    images[0].load();
    expect(onError).toHaveBeenCalledExactlyOnceWith("浏览器监控无法创建画布");
    painter.present(missing.canvas, frame(2));
    expect(images).toHaveLength(2);
  });
});
