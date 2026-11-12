export interface WebWaiter<T> {
  resolve: (value: T) => void;
  reject: (error: Error) => void;
}

/**
 * 浏览器链路的单次等待槽位（hello/connect/steps/relay.ready）。
 * 链路关闭或换连接时统一 reject，避免旧 Promise 悬挂。
 */
export class WebLinkWaiters {
  accepted: WebWaiter<Record<string, unknown>> | null = null;
  step2: WebWaiter<Uint8Array> | null = null;
  relayReady: WebWaiter<void> | null = null;

  rejectAll(error: Error): void {
    this.accepted?.reject(error);
    this.step2?.reject(error);
    this.relayReady?.reject(error);
    this.accepted = null;
    this.step2 = null;
    this.relayReady = null;
  }
}
