import { fork as nodeFork, type ForkOptions } from "node:child_process";
import type { Readable } from "node:stream";

import {
  mintBrokerSocketPath,
  probeHelperHealth,
  type HelperHealth,
} from "@mode/cua/broker";
// Contract constants are single-sourced from the producer package: locally re-declared literals here had drifted risk — a rename in the
// producer would silently orphan these. Kept as aliases for existing callers.
import { HELPER_ADDON_ENV, WINDOWS_DEV_CONTROL_PROTOCOL } from "@mode/cua/broker/server";

import type { ServiceLogger } from "#src/logger/serviceLogger.js";
import type { WindowsCuaRuntime } from "#src/cua-permission-broker/windowsCuaDevRuntime.js";

export const CONTROL_PROTOCOL = WINDOWS_DEV_CONTROL_PROTOCOL;
export const ADDON_ENV = HELPER_ADDON_ENV;

export interface WindowsCuaChild {
  readonly pid?: number;
  /**
   * `fork({silent:true})` 后可读的子进程流；stdio 继承路径（测试假件/旧适配）为
   * null/undefined——中继函数对空流空操作。
   */
  readonly stderr?: Readable | null;
  readonly stdout?: Readable | null;
  send(message: unknown): boolean;
  kill(): boolean;
  on(event: "message" | "error" | "exit", listener: (...args: unknown[]) => void): this;
  off(event: "message" | "error" | "exit", listener: (...args: unknown[]) => void): this;
}

export interface WindowsCuaChildProcessAdapter {
  /** command 是运行时 Node/Electron，argv[0] 是 Helper entry。 */
  fork(command: string, argv: string[], options: ForkOptions): WindowsCuaChild;
}

export interface WindowsCuaHelperHostOptions {
  runtime: WindowsCuaRuntime;
  childProcess?: WindowsCuaChildProcessAdapter;
  mintSocketPath?: () => string;
  mintPluginAuthority?: () => string;
  healthProbe?: (socketPath: string, timeoutMs: number) => Promise<HelperHealth>;
  startupTimeoutMs?: number;
  shutdownTimeoutMs?: number;
  logger?: ServiceLogger;
  /** Ready Helper 意外退出后通知生命周期 owner，仅触发 Helper recovery。 */
  onUnexpectedExit?: (info: { generation: number; pid: number | undefined }) => void;
}

export interface Generation {
  id: number;
  child: WindowsCuaChild;
  socketPath: string;
  stopped: boolean;
  exitObserved: boolean;
  abort?: (error: Error) => void;
  terminationPromise?: Promise<void>;
  removeMainListeners(): void;
}

export const defaultChildProcess: WindowsCuaChildProcessAdapter = {
  fork(command, argv, options) {
    const [entryPath, ...args] = argv;
    if (!entryPath) throw new Error("Windows Computer Use Helper entry path is required");
    return nodeFork(entryPath, args, { ...options, execPath: command });
  },
};

export const defaultSocketPathFactory = mintBrokerSocketPath;
export const defaultHealthProbe = (
  socketPath: string,
  timeoutMs: number,
): Promise<HelperHealth> => probeHelperHealth(socketPath, { timeoutMs });

/**
 * helper 子进程 stderr/stdout 接线（spec: lan-remote-desktop-native-media 失败语义
 * 「helper 侧 `[rd_media]` 运行告警必须接入主日志」）：Rust `warn()` 走 stderr eprintln，
 * 此前 stdio 继承下打包态无消费者、日志零落点（2026-10-13 排查实证）。按行中继：
 * - 含 `[rd_media]` 的行 → **warn 落盘**（GPU 合成回退/形状渲染失败等运行期降级，生产可见）；
 * - 其余 stderr 行 → debug（高频/开发诊断，按日志纪律不进生产盘）；
 * - stdout 仅排空（helper 契约走 IPC fork channel，内容不消费；不读会写满管道阻塞子进程）。
 * 行缓冲跨 chunk 拼接（`\r\n` 归一），流为 null（继承路径/假件）时空操作。
 */
export function relayHelperChildStdio(
  child: WindowsCuaChild,
  logger: ServiceLogger,
  context: { generation: number },
): void {
  const pid = child.pid;
  child.stdout?.resume();
  const stderr = child.stderr;
  if (!stderr) return;
  stderr.setEncoding?.("utf8");
  const emit = (line: string): void => {
    const trimmed = line.replace(/\r$/u, "").trimEnd();
    if (!trimmed) return;
    const meta = { source: "cua-helper-stderr", generation: context.generation, pid };
    if (trimmed.includes("[rd_media]")) {
      logger.warn(undefined, trimmed, meta);
    } else if (trimmed.includes("panicked at")) {
      // Rust panic 走 stderr 且不带 [rd_media] 前缀——必须升 error 落盘，
      // 否则 worker 线程死于 panic 在生产日志里零痕迹（2026-10-13 排查盲区）。
      logger.error(undefined, trimmed, meta);
    } else {
      logger.debug(undefined, trimmed, meta);
    }
  };
  let pending = "";
  stderr.on("data", (chunk: string | Buffer) => {
    pending += typeof chunk === "string" ? chunk : chunk.toString("utf8");
    let idx = pending.indexOf("\n");
    while (idx >= 0) {
      emit(pending.slice(0, idx));
      pending = pending.slice(idx + 1);
      idx = pending.indexOf("\n");
    }
  });
  // 进程退出前的无换行尾行不丢（eprintln 恒带 \n，这里只兜异常截断）。
  stderr.on("end", () => {
    if (pending) emit(pending);
    pending = "";
  });
}

export class WindowsCuaChildLifecycle {
  constructor(private readonly logger: ServiceLogger) {}

  async terminate(generation: Generation, context: string, timeoutMs: number): Promise<void> {
    if (generation.exitObserved) {
      generation.removeMainListeners();
      return;
    }
    // 先挂 wait listener 再发送 shutdown/kill，避免进程在 signal 的同一同步轮次退出而漏观测。
    const firstExit = this.waitForExit(generation, "first-wait", timeoutMs);
    this.sendShutdown(generation, context);
    if (await firstExit) {
      generation.removeMainListeners();
      return;
    }
    const secondExit = this.waitForExit(generation, "second-wait", timeoutMs);
    this.kill(generation, context);
    if (await secondExit) {
      generation.removeMainListeners();
      return;
    }
    const blocker = new Error(
      "Windows Computer Use Helper termination blocker: child did not exit after kill",
    );
    this.logFailure(generation.id, generation.child.pid, `${context}-termination-blocker`, blocker);
    throw blocker;
  }

  sendShutdown(generation: Generation, context: string): void {
    try {
      generation.child.send({ protocol: CONTROL_PROTOCOL, type: "shutdown" });
    } catch (error) {
      this.logFailure(generation.id, generation.child.pid, `${context}-send`, error);
    }
  }

  kill(generation: Generation, context: string): void {
    try {
      generation.child.kill();
    } catch (error) {
      this.logFailure(generation.id, generation.child.pid, `${context}-kill`, error);
    }
  }

  on(
    child: WindowsCuaChild,
    event: "message" | "error" | "exit",
    listener: (...args: unknown[]) => void,
    generation: number,
    errorClass: string,
  ): boolean {
    try {
      child.on(event, listener);
      return true;
    } catch (error) {
      this.logFailure(generation, child.pid, errorClass, error);
      return false;
    }
  }

  off(
    child: WindowsCuaChild,
    event: "message" | "error" | "exit",
    listener: (...args: unknown[]) => void,
    generation: number,
    errorClass: string,
  ): void {
    try {
      child.off(event, listener);
    } catch (error) {
      this.logFailure(generation, child.pid, errorClass, error);
    }
  }

  logFailure(
    generation: number,
    pid: number | undefined,
    errorClass: string,
    error: unknown,
  ): void {
    this.logger.warn(undefined, "Windows Computer Use Helper lifecycle failure", {
      generation,
      pid,
      errorClass,
      error: error instanceof Error ? error.name : typeof error,
    });
  }

  private waitForExit(
    generation: Generation,
    errorClass: string,
    timeoutMs: number,
  ): Promise<boolean> {
    if (generation.exitObserved) return Promise.resolve(true);
    return new Promise((resolve) => {
      let done = false;
      const finish = (exited: boolean) => {
        if (done) return;
        done = true;
        clearTimeout(timer);
        this.off(generation.child, "exit", onExit, generation.id, `${errorClass}-off`);
        resolve(exited);
      };
      const onExit = () => {
        generation.exitObserved = true;
        finish(true);
      };
      const timer = setTimeout(() => finish(false), timeoutMs);
      timer.unref?.();
      if (!this.on(generation.child, "exit", onExit, generation.id, `${errorClass}-on`))
        finish(false);
      // send() 后、临时 listener 注册前 child 可能已经触发主 exit listener；再次读取
      // generation 事实，避免把已退出 H1 错当成超时并留下不存在的 termination blocker。
      else if (generation.exitObserved) finish(true);
    });
  }
}

export type WindowsDevHelperControlMessage =
  | { protocol: typeof CONTROL_PROTOCOL; type: "transport_ready"; socketPath: string; pid: number }
  | { protocol: typeof CONTROL_PROTOCOL; type: "ready"; socketPath: string; pid: number }
  | { protocol: typeof CONTROL_PROTOCOL; type: "error"; message: string };

/**
 * Parse a `mode-cua-windows-dev/v1` control message from the helper child.
 *
 * "transport_ready" means the named pipe is bound and authenticated, while
 * "ready" remains the full health handshake. "error" is the helper's OWN startup-failure
 * report (windowsDevHelperMain.ts sends `{protocol, type:"error", message}`
 * before exiting 1/2) — previously this fell through to null and the host
 * misclassified it as "Invalid Windows Computer Use Helper control message"
 * (malformed-message), discarding the diagnostic string.
 * Any future additive message type parses as "ignore" (forward-compatible)
 * instead of failing the generation.
 */
export function parseReadyMessage(
  message: unknown,
): WindowsDevHelperControlMessage | "ignore" | null {
  if (!message || typeof message !== "object") return null;
  const value = message as Record<string, unknown>;
  if (value.protocol !== CONTROL_PROTOCOL) return "ignore";
  if (value.type === "error") {
    if (typeof value.message !== "string" || value.message.length === 0) {
      return null;
    }
    return { protocol: CONTROL_PROTOCOL, type: "error", message: value.message };
  }
  if (value.type !== "ready" && value.type !== "transport_ready") return "ignore";
  if (
    typeof value.socketPath !== "string" ||
    !value.socketPath ||
    !Number.isInteger(value.pid) ||
    (value.pid as number) <= 0
  )
    return null;
  return {
    protocol: CONTROL_PROTOCOL,
    type: value.type,
    socketPath: value.socketPath,
    pid: value.pid as number,
  };
}

export function isExactHealthPid(
  health: HelperHealth,
  childPid: number | undefined,
): health is HelperHealth & { pid: number } {
  return Number.isInteger(health.pid) && (health.pid as number) > 0 && health.pid === childPid;
}

export function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}
