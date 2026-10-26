import { MessagePortProtocol, ChannelClient } from "@mode/rpc";
import type { Event } from "@mode/rpc";
import type { IServiceAccessor } from "@mode/services";
import { RemoteServiceAccess } from "./remoteServiceAccess.js";
import { isRendererProductionBuild } from "./rendererLoggingEnv.js";

function logMessagePortDebug(message: string): void {
  // 生产构建下 renderer 连接日志不输出，避免窗口启动和重连路径产生同步 console 成本。
  if (isRendererProductionBuild()) {
    return;
  }
  console.log(message);
}

export interface MessagePortServiceConnection {
  services: IServiceAccessor;
  dispose: (reason?: Error) => void;
  /**
   * Host 活性探测（半开管道心跳，spec `tauri-renderer-host.md` §22.5）：对 `host` 频道发
   * `ping` 并限时等 `pong`。进程死亡走 stdout EOF（既有路径）；「进程活着但事件循环停摆」
   * 只有应用层探测能发现。超时/形状不对都以 reject 收场，由调用方计 strike。
   */
  pingHost: (timeoutMs: number) => Promise<void>;
}

/** 与 host-standalone 的 `HOST_CHANNEL` 同一字面量（跨包契约只看字符串，不引实现）。 */
const HOST_CHANNEL_NAME = "host";

/**
 * 创建一个带明确生命周期的 MessagePort service connection。
 *
 * scoped remote session 换代时必须同时释放 ChannelClient 和底层 port，
 * 否则旧 attachment 上的挂起 RPC 无法 settle，并会继续占用上层去重状态。
 */
export function createMessagePortServiceConnection(
  port: MessagePort,
): MessagePortServiceConnection {
  logMessagePortDebug("[messageport] creating protocol and client...");
  const protocol = new MessagePortProtocol(port);
  const client = new ChannelClient(protocol);
  client.onDidInitialize(() => {
    logMessagePortDebug("[messageport] ChannelClient received Initialize from server");
  });
  logMessagePortDebug("[messageport] client created, waiting for Initialize...");

  const services = new RemoteServiceAccess(client);
  let disposed = false;
  return {
    services,
    pingHost: (timeoutMs: number): Promise<void> => {
      const channel = client.getChannel<{
        call: <R>(command: string, arg?: unknown) => Promise<R>;
        listen: <R>(event: string, arg?: unknown) => Event<R>;
      }>(HOST_CHANNEL_NAME);
      let timer: ReturnType<typeof setTimeout> | undefined;
      const deadline = new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error("host ping timeout")), timeoutMs);
      });
      return Promise.race([
        channel.call<{ pong?: boolean }>("ping").then((result) => {
          if (result?.pong !== true) {
            throw new Error("host ping malformed response");
          }
        }),
        deadline,
      ]).finally(() => {
        if (timer !== undefined) {
          clearTimeout(timer);
        }
      });
    },
    dispose: (reason?: Error) => {
      if (disposed) {
        return;
      }
      disposed = true;
      client.dispose(reason);
      protocol.disconnect();
    },
  };
}

/**
 * 通过 MessagePort 连接服务。
 *
 * Desktop 模式下，utilityProcess（或 main 进程的远程代理）通过 MessagePort
 * 暴露 ChannelServer，renderer 用此函数建立 ChannelClient 连接。
 */
export function connectViaMessagePort(port: MessagePort): IServiceAccessor {
  return createMessagePortServiceConnection(port).services;
}
