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
import { mkdir, rm, writeFile } from "node:fs/promises";
import { createServer, type Server, type Socket } from "node:net";
import { dirname, join } from "node:path";

const MAX_MESSAGE_BYTES = 1024 * 1024;
const MAX_COOKIES = 100_000;
const DEFAULT_TTL_MS = 60_000;

export type BrowserImportCookie = Record<string, unknown>;

export interface BrowserImportChannelLogger {
  info: (...args: unknown[]) => void;
  warn: (...args: unknown[]) => void;
}

export interface BrowserImportChannelOptions {
  /** 收到一组合法 cookie 后的写入回调；返回写入条数。 */
  onCookies: (cookies: BrowserImportCookie[]) => Promise<number> | number;
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
  const server = createServer((socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
    void handleConnection(socket, token, options, ttlMs);
  });
  await listen(server, pipe);

  let closed = false;
  const close = async (): Promise<void> => {
    if (closed) return;
    closed = true;
    for (const socket of sockets) socket.destroy();
    sockets.clear();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await rm(channelPath, { force: true });
  };
  const timer = setTimeout(() => {
    void close();
  }, ttlMs);
  timer.unref?.();

  options.logger?.info("[browser-import] channel ready", { channelPath, expiresAt });
  return { channelPath, close };
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
    if (!hello || hello.v !== 1 || hello.kind !== "hello" || !tokenMatches(hello.token, expectedToken)) {
      writeFrame(socket, { ok: false, error: "channel_token_mismatch" });
      return;
    }
    writeFrame(socket, { ok: true });

    const payload = await reader.next(ttlMs);
    const cookies = validateCookies(payload);
    if (!cookies) {
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
      writeFrame(socket, { ok: false, error: "cookie_write_failed" });
      return;
    }
    options.logger?.info("[browser-import] imported cookies", { count });
    writeFrame(socket, { ok: true, count });
  } catch (error) {
    options.logger?.warn("[browser-import] channel failed", {
      reason: error instanceof Error ? error.message : String(error),
    });
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

function createFrameReader(socket: Socket): { next: (timeoutMs: number) => Promise<Frame | null> } {
  let buffered = Buffer.alloc(0);
  const frames: Frame[] = [];
  const waiters: Array<(value: Frame | null) => void> = [];

  socket.on("data", (chunk: Buffer) => {
    buffered = Buffer.concat([buffered, chunk]);
    flush();
  });

  function deliver(value: Frame | null): void {
    const waiter = waiters.shift();
    if (waiter) waiter(value);
    else if (value) frames.push(value);
  }

  function flush(): void {
    while (buffered.length >= 4) {
      const length = buffered.readUInt32LE(0);
      if (length === 0 || length > MAX_MESSAGE_BYTES) {
        buffered = Buffer.alloc(0);
        for (const waiter of waiters.splice(0)) waiter(null);
        return;
      }
      if (buffered.length < 4 + length) return;
      const payload = buffered.subarray(4, 4 + length);
      buffered = buffered.subarray(4 + length);
      try {
        deliver(JSON.parse(payload.toString("utf8")) as Frame);
      } catch {
        deliver(null);
      }
    }
  }

  return {
    next(timeoutMs: number): Promise<Frame | null> {
      const queued = frames.shift();
      if (queued) return Promise.resolve(queued);
      return new Promise((resolve) => {
        let timer: NodeJS.Timeout;
        const handler = (value: Frame | null): void => {
          clearTimeout(timer);
          resolve(value);
        };
        timer = setTimeout(() => {
          const index = waiters.indexOf(handler);
          if (index >= 0) waiters.splice(index, 1);
          resolve(null);
        }, timeoutMs);
        waiters.push(handler);
        flush();
      });
    },
  };
}
