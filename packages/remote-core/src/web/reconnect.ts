/** 断线重连退避：指数增长 + 抖动，超过上限则放弃（调用方决定是否关闭链路）。 */
export class ReconnectScheduler {
  private timer: ReturnType<typeof setTimeout> | null = null;
  private attempts = 0;
  private readonly options: {
    baseMs?: number;
    maxAttempts?: number;
    schedule: (delayMs: number) => void;
    giveUp: () => void;
  };

  constructor(options: ReconnectScheduler["options"]) {
    this.options = options;
  }

  /** 成功建立后清零，下一次断线重新从最小退避开始。 */
  reset(): void {
    this.attempts = 0;
  }

  schedule(): void {
    if (this.timer) return;
    this.attempts += 1;
    if (this.attempts > (this.options.maxAttempts ?? 10)) {
      this.options.giveUp();
      return;
    }
    const base = Math.min(
      (this.options.baseMs ?? 500) * 2 ** (this.attempts - 1),
      5_000,
    );
    const delay = Math.max(300, Math.round(base * (0.5 + Math.random() * 0.5)));
    this.timer = setTimeout(() => {
      this.timer = null;
      this.options.schedule(delay);
    }, delay);
  }

  cancel(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }
}
