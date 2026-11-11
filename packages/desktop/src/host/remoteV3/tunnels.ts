import { randomUUID } from "node:crypto";

import { decodeBase64Url, encodeBase64Url, type RemoteStream } from "@mode/remote-core";
import type { RemoteV3TunnelEvent, RemoteV3TunnelOpenParams } from "@mode/shared";

export type TunnelStreamOpener = (
  hostDeviceId: string,
  service: string,
  metadata: Record<string, unknown>,
) => Promise<RemoteStream>;

/** 本机作为 Client 的隧道集合：stream 生命周期与 RPC 响应事件分离。 */
export class RemoteV3TunnelRuntime {
  private readonly openStream: TunnelStreamOpener;
  private readonly tunnels = new Map<string, RemoteStream>();
  private readonly listeners = new Set<(event: RemoteV3TunnelEvent) => void>();
  // 渲染进程先 openTunnel 再订阅 onTunnelEvent，中间隔一次 RPC；没有监听者时
  // 先把事件缓存起来，首个订阅者补齐，避免 Initialize/首帧在这段窗口里丢失。
  private readonly pendingEvents: RemoteV3TunnelEvent[] = [];

  constructor(openStream: TunnelStreamOpener) {
    this.openStream = openStream;
  }

  async open(params: RemoteV3TunnelOpenParams): Promise<{ tunnelId: string }> {
    const stream = await this.openStream(params.hostDeviceId, "remote.ws", {
      path: params.path,
      ...(params.headers ? { headers: params.headers } : {}),
    });
    const tunnelId = randomUUID();
    this.tunnels.set(tunnelId, stream);
    stream.onData((data) => {
      this.emit({ tunnelId, kind: "message", data: encodeBase64Url(data) });
    });
    stream.onClose((reason) => {
      this.tunnels.delete(tunnelId);
      this.emit({ tunnelId, kind: "close", reason });
    });
    this.emit({ tunnelId, kind: "open" });
    return { tunnelId };
  }

  async send(tunnelId: string, data: string): Promise<void> {
    const stream = this.tunnels.get(tunnelId);
    if (!stream) throw new Error("隧道不存在或已关闭");
    stream.send(decodeBase64Url(data, "tunnel.data"));
  }

  async close(tunnelId: string): Promise<void> {
    const stream = this.tunnels.get(tunnelId);
    if (!stream) return;
    this.tunnels.delete(tunnelId);
    stream.close("closed by controller");
  }

  closeAll(): void {
    for (const [tunnelId, stream] of this.tunnels) {
      this.tunnels.delete(tunnelId);
      stream.close("client disposed");
    }
  }

  onEvent(listener: (event: RemoteV3TunnelEvent) => void): { dispose(): void } {
    const first = this.listeners.size === 0;
    this.listeners.add(listener);
    if (first) {
      for (const pending of this.pendingEvents.splice(0, this.pendingEvents.length)) {
        listener(pending);
      }
    }
    return {
      dispose: () => {
        this.listeners.delete(listener);
      },
    };
  }

  private emit(event: RemoteV3TunnelEvent): void {
    if (this.listeners.size === 0) {
      if (this.pendingEvents.length < 256) this.pendingEvents.push(event);
      return;
    }
    for (const listener of this.listeners) listener(event);
  }
}
