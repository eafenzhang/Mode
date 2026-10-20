/**
 * LAN 监听的「等到 listen 完成」helper。
 *
 * 为什么单独成文件：`@hono/node-server` 的 `serve()` 同步返回 Node http.Server，
 * 端口被占用时通过 **`error` 事件**异步报告，而不是让任何 Promise reject。没有监听者时，
 * 这个事件会升级成未捕获异常并杀死 Host 进程；调用方只能看到 `transport_closed`，
 * 看起来像「数据准备进程意外退出」。本 helper 把 listening / error 两条事件收成
 * resolve / reject，让端口冲突回到「本次 LAN 不可用」的产品语义。
 *
 * 这里只依赖结构面（`ListenableServer`），不把 `@hono/node-server` 的具体类型带进 Host 模块。
 */

export type ListenableServer = {
  readonly listening: boolean;
  address(): unknown;
  once(event: "listening", listener: () => void): unknown;
  once(event: "error", listener: (error: Error) => void): unknown;
  off(event: "listening", listener: () => void): unknown;
  off(event: "error", listener: (error: Error) => void): unknown;
  on(event: "error", listener: (error: Error) => void): unknown;
  close(callback?: (error?: Error) => void): unknown;
};

/**
 * 等 listen 完成并返回实际端口（`port: 0` 时取系统分配值）。
 * 监听错误以 `局域网端口 <port> 监听失败：<原因>` 的 rejection 返回。
 */
/** 监听失败后的 best-effort 释放：未进入 listening 的 close 可能抛错，原始错误优先。 */
export function closeListenableQuietly(server: ListenableServer): void {
  try {
    server.close();
  } catch {
    /* 未进入监听状态的 close 可能抛错：忽略。 */
  }
}

export async function waitForListenPort(
  server: ListenableServer,
  fallbackPort: number,
  requestedPort: number,
): Promise<number> {
  return await new Promise<number>((resolve, reject) => {
    let settled = false;
    const cleanup = (): void => {
      server.off("listening", onListening);
      server.off("error", onError);
    };
    const readPort = (): number => {
      const address = server.address();
      if (address && typeof address === "object" && "port" in address) {
        const port = Number(address.port);
        if (Number.isFinite(port) && port > 0) {
          return port;
        }
      }
      return fallbackPort;
    };
    const onListening = (): void => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(readPort());
    };
    const onError = (error: Error): void => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(new Error(`局域网端口 ${requestedPort} 监听失败：${error.message}`));
    };
    server.once("listening", onListening);
    server.once("error", onError);
    // listen 可能在监听器挂载前就已完成（事件已派发）；补一次同步检查。
    if (server.listening) onListening();
  });
}
