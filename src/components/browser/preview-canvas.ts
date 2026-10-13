import type { BrowserFrameEvent } from "../../types";

/** 每个视图最多解码一张图，并且只排队最新帧；旧解码不能覆盖新会话。 */
export class PreviewCanvas {
  private canvas: HTMLCanvasElement | null = null;
  private latest: BrowserFrameEvent | null = null;
  private drawn: BrowserFrameEvent | null = null;
  private pending: HTMLImageElement | null = null;
  private generation = 0;

  constructor(
    private readonly onError: (message: string) => void,
    private readonly createImage: () => HTMLImageElement = () => new Image(),
  ) {}

  present(canvas: HTMLCanvasElement, frame: BrowserFrameEvent): void {
    if (this.canvas !== canvas) {
      this.pause();
      this.canvas = canvas;
    }
    this.latest = frame;
    if (!this.pending && this.drawn !== frame) this.decode();
  }

  pause(): void {
    this.generation += 1;
    if (this.pending) {
      this.pending.onload = null;
      this.pending.onerror = null;
      this.pending.src = "";
    }
    this.pending = null;
    this.latest = null;
    this.drawn = null;
  }

  clear(canvas: HTMLCanvasElement | null = this.canvas): void {
    this.pause();
    this.canvas = canvas;
    if (canvas) {
      canvas.width = 0;
      canvas.height = 0;
    }
  }

  private decode(): void {
    const frame = this.latest;
    const canvas = this.canvas;
    if (!frame || !canvas) return;
    const generation = this.generation;
    const image = this.createImage();
    this.pending = image;
    const complete = () => {
      image.onload = null;
      image.onerror = null;
      if (this.generation !== generation) return;
      this.pending = null;
      if (this.latest !== frame) this.decode();
    };
    image.onload = () => {
      if (this.generation !== generation) return;
      if (this.latest === frame) {
        const context = canvas.getContext("2d");
        if (context) {
          canvas.width = frame.width;
          canvas.height = frame.height;
          context.drawImage(image, 0, 0, frame.width, frame.height);
          this.drawn = frame;
        } else {
          this.onError("浏览器监控无法创建画布");
        }
      }
      complete();
    };
    image.onerror = () => {
      if (this.generation === generation && this.latest === frame) {
        this.onError("浏览器监控画面解码失败");
      }
      complete();
    };
    image.src = frame.data;
  }
}
