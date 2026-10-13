/** 合并并串行发送观察者状态，防止隐藏后的迟到续租重新开启采集。 */
export class PreviewLease {
  private visible = false;
  private disposed = false;
  private dirty = false;
  private sending = false;
  private timer: ReturnType<typeof setInterval> | undefined;

  constructor(
    private readonly send: (visible: boolean) => Promise<void>,
    private readonly onError: (error: string | null) => void,
  ) {}

  setVisible(visible: boolean): void {
    if (this.disposed || this.visible === visible) return;
    this.visible = visible;
    clearInterval(this.timer);
    this.timer = visible ? setInterval(() => this.enqueue(), 3_000) : undefined;
    this.enqueue();
  }

  dispose(): void {
    if (this.disposed) return;
    clearInterval(this.timer);
    // 即使上一条注册仍在路上，最终发送的状态也必须是关闭。
    if (this.visible || this.sending) {
      this.visible = false;
      this.enqueue();
    }
    this.disposed = true;
  }

  private enqueue(): void {
    this.dirty = true;
    void this.flush();
  }

  private async flush(): Promise<void> {
    if (this.sending) return;
    this.sending = true;
    try {
      while (this.dirty) {
        this.dirty = false;
        try {
          await this.send(this.visible);
          if (!this.disposed) this.onError(null);
        } catch (error) {
          const message = `浏览器监控订阅失败：${String(error)}`;
          if (!this.disposed) this.onError(message);
          else console.warn(message);
        }
      }
    } finally {
      this.sending = false;
    }
  }
}
