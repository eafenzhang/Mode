import type { ConnectAcceptedPayload } from "../domain/controlMessages.js";
import type { ControlFrame } from "../domain/controlFrames.js";
import { RemoteError } from "../domain/errors.js";

/** 按谓词等待 Control 帧（握手、P2P answer 等），超时统一 reject。 */
export class FrameWaiters {
  private readonly entries: Array<{
    predicate: (frame: ControlFrame) => boolean;
    resolve: (frame: ControlFrame) => void;
    reject: (error: Error) => void;
    timer: NodeJS.Timeout;
  }> = [];

  wait(
    predicate: (frame: ControlFrame) => boolean,
    timeoutMs: number,
    message = "等待 control frame 超时",
  ): Promise<ControlFrame> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        const index = this.entries.findIndex((entry) => entry.timer === timer);
        if (index >= 0) this.entries.splice(index, 1);
        reject(new RemoteError("timeout", message, { retryable: true }));
      }, timeoutMs);
      timer.unref?.();
      this.entries.push({ predicate, resolve, reject, timer });
    });
  }

  dispatch(frame: ControlFrame): void {
    for (let index = this.entries.length - 1; index >= 0; index -= 1) {
      const waiter = this.entries[index]!;
      if (!waiter.predicate(frame)) continue;
      this.entries.splice(index, 1);
      clearTimeout(waiter.timer);
      waiter.resolve(frame);
    }
  }

  rejectAll(error: Error): void {
    for (const waiter of this.entries.splice(0, this.entries.length)) {
      clearTimeout(waiter.timer);
      waiter.reject(error);
    }
  }
}

/** `connect.accepted` 等待表：按 Host deviceId 挂起，链路关闭时统一 reject。 */
export class AcceptedWaiters {
  private readonly entries = new Map<
    string,
    { resolve: (payload: ConnectAcceptedPayload) => void; reject: (error: Error) => void }
  >();

  wait(hostDeviceId: string, timeoutMs: number): Promise<ConnectAcceptedPayload> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.entries.delete(hostDeviceId);
        reject(
          new RemoteError("connect_timeout", "等待 connect.accepted 超时", { retryable: true }),
        );
      }, timeoutMs);
      timer.unref?.();
      this.entries.set(hostDeviceId, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
    });
  }

  resolve(hostDeviceId: string, payload: ConnectAcceptedPayload): void {
    const waiter = this.entries.get(hostDeviceId);
    if (!waiter) return;
    this.entries.delete(hostDeviceId);
    waiter.resolve(payload);
  }

  rejectAll(error: Error): void {
    for (const waiter of this.entries.values()) waiter.reject(error);
    this.entries.clear();
  }
}
