// Mode main 侧的浏览器导入通道（P1/T4）。
//
// 规格：`docs/specs/chrome-browser-import-extension.md` §3、§4.2、§5。
//
// 生命周期：一次导入 = 一个短生命周期命名管道 + 一次性 token + channel.json；TTL 到期自动关闭
// 并删除 channel.json。扩展 popup 由用户点击触发；本模块只负责接收与校验，不主动读取 Chrome。
//
// 安全（R1–R10）：token 一次性、TTL ≤60s、channel.json 在 `%LOCALAPPDATA%\Mode\browser-import`
// 下（继承当前用户 ACL）、日志只记数量/reason、不记 cookie 值与 token。
import { randomBytes, timingSafeEqual } from "node:crypto";
import { rmSync } from "node:fs";
import { mkdir, rm, writeFile } from "node:fs/promises";
import { createServer, type Server, type Socket } from "node:net";
import { dirname, join } from "node:path";

const MAX_MESSAGE_BYTES = 1024 * 1024;
const MAX_COOKIES = 100_000;
const DEFAULT_TTL_MS = 60_000;

export type BrowserImportCookie = Record<string, unknown>;

/** 本通道能判定的终止错误码；与 spec §5 的稳定码一致。 */
export type BrowserImportChannelErrorCode =
  | "channel_timeout"
  | "channel_token_mismatch"
  | "payload_too_large"
  | "invalid_payload"
  | "cookie_write_failed"
  | "extension_missing";

export interface BrowserImportChannelLogger {
  info: (...args: unknown[]) => void;
  warn: (...args: unknown[]) => void;
}

export interface BrowserImportChannelOptions {
  /** 收到一组合法 cookie 后的写入回调；返回写入条数。 */
  onCookies: (cookies: BrowserImportCookie[]) => Promise<number> | number;
  /**
   * 通道在本次导入结束前判定失败时调用一次（token 不匹配、payload 非法/超限、
   * 写入失败、TTL 到期）。调用方据此快速失败，而不是等满超时。
   */
  onError?: (code: BrowserImportChannelErrorCode) => void;
  /** 扩展 popup 在未完成导入时关闭（Native host 转发 cancel）。 */
  onCancel?: () => void;
  /** 测试/诊断可覆盖 channel.json 路径。 */
  channelPath?: string;
  /** 默认 60s；规格上限也是 60s。 */
  ttlMs?: number;
  logger?: BrowserImportChannelLogger;
}

export interface BrowserImportChannel {
  channelPath: string;
  /** 关闭服务、删除 channel.json；幂等。 */
  close: () => Promise<void>;
  /** 进程即将退出时的同步清理：只删 channel.json 并断开套接字，不等待 server 关闭。 */
  dispose: () => void;
}

type Frame = { v?: number; kind?: string; token?: string; cookies?: unknown };

/**
 * 创建一个单次导入通道。调用方应在用户触发导入后创建，并在完成后 `close()`。
 */
export async function createBrowserImportChannel(
  options: BrowserImportChannelOptions,
): Promise<BrowserImportChannel> {
  const ttlMs = Math.min(options.ttlMs ?? DEFAULT_TTL_MS, DEFAULT_TTL_MS);
  const token = randomBytes(32).toString("hex");
  const pipe = `\\\\.\\pipe\\mode-browser-import-${randomBytes(12).toString("hex")}`;
  const channelPath =
    options.channelPath ??
    join(
      process.env.LOCALAPPDATA ?? process.env.TMP ?? process.cwd(),
      "Mode",
      "browser-import",
      "channel.json",
    );
  const expiresAt = Date.now() + ttlMs;

  await mkdir(dirname(channelPath), { recursive: true });
  await writeFile(
    channelPath,
    JSON.stringify({ v: 1, pipe, token, expiresAt }),
    { encoding: "utf8" },
  );

  const sockets = new Set<Socket>();
  let sawConnection = false;
  const server = createServer((socket) => {
    sawConnection = true;
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
    // 客户端（native host）异常退出会产生 error 事件；没有监听器时 Node 会把它升级为
    // 未捕获异常并终止 main 进程。这里只记 reason，不记录 token/payload。
    socket.on("error", (error) => {
      options.logger?.warn("[browser-import] channel socket error", {
        reason: error instanceof Error ? error.message : String(error),
      });
    });
    void handleConnection(socket, token, options, ttlMs);
  });
  await listen(server, pipe);
  // listen 成功后拒绝用的临时监听器已移除；之后的服务端错误同样不能变成未捕获异常。
  server.on("error", (error) => {
    options.logger?.warn("[browser-import] channel server error", {
      reason: error instanceof Error ? error.message : String(error),
    });
  });

  let closed = false;
  const close = async (): Promise<void> => {
    if (closed) return;
    closed = true;
    clearTimeout(timer);
    for (const socket of sockets) socket.destroy();
    sockets.clear();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await rm(channelPath, { force: true });
  };
  const timer = setTimeout(() => {
    // 没有任何客户端连接时，更可能是扩展未安装/未启用；连接过但迟迟没有合法帧才是通道超时。
    options.onError?.(sawConnection ? "channel_timeout" : "extension_missing");
    void close();
  }, ttlMs);
  timer.unref?.();

  const dispose = (): void => {
    if (closed) return;
    closed = true;
    clearTimeout(timer);
    for (const socket of sockets) socket.destroy();
    sockets.clear();
    try {
      server.close();
    } catch {
      // 进程退出路径上不处理 server 关闭错误。
    }
    try {
      rmSync(channelPath, { force: true });
    } catch {
      // 文件已被监听器删除或目录不可写时忽略，进程退出不阻塞。
    }
  };

  options.logger?.info("[browser-import] channel ready", { channelPath, expiresAt });
  return { channelPath, close, dispose };
}

function listen(server: Server, pipe: string): Promise<void> {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(pipe, () => {
      server.off("error", reject);
      resolve();
    });
  });
}

async function handleConnection(
  socket: Socket,
  expectedToken: string,
  options: BrowserImportChannelOptions,
  ttlMs: number,
): Promise<void> {
  const reader = createFrameReader(socket);
  try {
    const hello = await reader.next(ttlMs);
    if (!hello.ok) {
      // 超时通常意味着客户端已断开，不必再回帧；形状错误则明确回复稳定码。
      if (hello.error !== "channel_timeout") {
        options.onError?.(hello.error);
        writeFrame(socket, { ok: false, error: hello.error });
      }
      return;
    }
    if (
      hello.value.v !== 1 ||
      hello.value.kind !== "hello" ||
      !tokenMatches(hello.value.token, expectedToken)
    ) {
      options.onError?.("channel_token_mismatch");
      writeFrame(socket, { ok: false, error: "channel_token_mismatch" });
      return;
    }
    writeFrame(socket, { ok: true });

    const payload = await reader.next(ttlMs);
    if (!payload.ok) {
      if (payload.error !== "channel_timeout") {
        options.onError?.(payload.error);
        writeFrame(socket, { ok: false, error: payload.error });
      }
      return;
    }
    if (payload.value.v === 1 && payload.value.kind === "cancel") {
      options.logger?.info("[browser-import] import cancelled by extension");
      options.onCancel?.();
      writeFrame(socket, { ok: true });
      return;
    }
    const cookies = validateCookies(payload.value);
    if (!cookies) {
      options.onError?.("invalid_payload");
      writeFrame(socket, { ok: false, error: "invalid_payload" });
      return;
    }
    let count: number;
    try {
      count = await options.onCookies(cookies);
    } catch (error) {
      options.logger?.warn("[browser-import] cookie write failed", {
        reason: error instanceof Error ? error.message : String(error),
      });
      options.onError?.("cookie_write_failed");
      writeFrame(socket, { ok: false, error: "cookie_write_failed" });
      return;
    }
    options.logger?.info("[browser-import] imported cookies", { count });
    writeFrame(socket, { ok: true, count });
  } catch (error) {
    options.logger?.warn("[browser-import] channel failed", {
      reason: error instanceof Error ? error.message : String(error),
    });
    options.onError?.("invalid_payload");
    writeFrame(socket, { ok: false, error: "invalid_payload" });
  } finally {
    socket.end();
  }
}

function tokenMatches(given: unknown, expected: string): boolean {
  if (typeof given !== "string") return false;
  const givenBytes = Buffer.from(given, "utf8");
  const expectedBytes = Buffer.from(expected, "utf8");
  return givenBytes.length === expectedBytes.length && timingSafeEqual(givenBytes, expectedBytes);
}

function validateCookies(payload: Frame | null): BrowserImportCookie[] | null {
  if (!payload || payload.v !== 1 || payload.kind !== "cookies") return null;
  if (!Array.isArray(payload.cookies) || payload.cookies.length > MAX_COOKIES) return null;
  for (const cookie of payload.cookies) {
    if (!cookie || typeof cookie !== "object" || Array.isArray(cookie)) return null;
    const record = cookie as Record<string, unknown>;
    if (
      typeof record.name !== "string" ||
      typeof record.value !== "string" ||
      typeof record.domain !== "string" ||
      typeof record.path !== "string"
    ) {
      return null;
    }
  }
  return payload.cookies as BrowserImportCookie[];
}

function writeFrame(socket: Socket, value: unknown): void {
  const payload = Buffer.from(JSON.stringify(value), "utf8");
  if (payload.length === 0 || payload.length > MAX_MESSAGE_BYTES) return;
  const frame = Buffer.allocUnsafe(4 + payload.length);
  frame.writeUInt32LE(payload.length, 0);
  payload.copy(frame, 4);
  if (!socket.destroyed) socket.write(frame);
}

function createFrameReader(socket: Socket): {
  next: (
    timeoutMs: number,
  ) => Promise<
    | { ok: true; value: Frame }
    | { ok: false; error: "channel_timeout" | "payload_too_large" | "invalid_payload" }
  >;
} {
  let buffered = Buffer.alloc(0);
  const frames: Frame[] = [];
  const waiters: Array<(result: FrameReadResult) => void> = [];

  socket.on("data", (chunk: Buffer) => {
    buffered = Buffer.concat([buffered, chunk]);
    flush();
  });

  function deliver(result: FrameReadResult): void {
    const waiter = waiters.shift();
    if (waiter) waiter(result);
    else if (result.ok) frames.push(result.value);
  }

  function flush(): void {
    while (buffered.length >= 4) {
      const length = buffered.readUInt32LE(0);
      if (length === 0 || length > MAX_MESSAGE_BYTES) {
        // 长度前缀非法或超限都视为协议违规：清空缓冲并把当前等待者全部判失败。
        buffered = Buffer.alloc(0);
        for (const waiter of waiters.splice(0)) waiter({ ok: false, error: "payload_too_large" });
        return;
      }
      if (buffered.length < 4 + length) return;
      const payload = buffered.subarray(4, 4 + length);
      buffered = buffered.subarray(4 + length);
      try {
        deliver({ ok: true, value: JSON.parse(payload.toString("utf8")) as Frame });
      } catch {
        deliver({ ok: false, error: "invalid_payload" });
      }
    }
  }

  return {
    next(timeoutMs: number): Promise<FrameReadResult> {
      const queued = frames.shift();
      if (queued) return Promise.resolve({ ok: true, value: queued });
      return new Promise((resolve) => {
        let timer: NodeJS.Timeout;
        const handler = (result: FrameReadResult): void => {
          clearTimeout(timer);
          resolve(result);
        };
        timer = setTimeout(() => {
          const index = waiters.indexOf(handler);
          if (index >= 0) waiters.splice(index, 1);
          resolve({ ok: false, error: "channel_timeout" });
        }, timeoutMs);
        waiters.push(handler);
        flush();
      });
    },
  };
}

type FrameReadResult =
  | { ok: true; value: Frame }
  | { ok: false; error: "channel_timeout" | "payload_too_large" | "invalid_payload" };
