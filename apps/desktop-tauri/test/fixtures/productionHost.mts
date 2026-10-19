/**
 * 起**生产 Host sidecar**（`packages/desktop/src/host-standalone/index.ts`，壳 spawn 的那份）并把
 * 「页面 port ↔ Host stdio」按生产口径对接，供 Task 2 的用例复用。
 *
 * 与 Task 1 的 `portRoundTrip.test.mts` 内联帮手的关系：那份守着 Task 1 自己的交付物，
 * 这份是 Task 2 多个用例共享的驱动面（原生依赖 / 真服务面 / listen 守卫 / startup 映射端到端），
 * 抽出来是为了让每个用例只写**自己的断言**，而不是各抄一份 spawn + 编解码（抄三份就是三个漂移源）。
 * 两份的协议口径必须一致：出方向原样写帧（客户端帧本来就是 `Uint8Array`），入方向**包成
 * `Uint8Array`** 再投端口（Tauri 的 raw 响应是 `ArrayBuffer`，不包会被 `MessagePortProtocol`
 * 静默丢弃，spike §4.3）。
 *
 * 数据隔离：调用方必须给 `home`，可选 `dataBaseDir`；两条覆写都由本模块注入子进程环境。
 * 不给隔离目录的用例不该存在（Global Constraints：测试绝不碰真实 `~/.mode`）。
 */
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { rmSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { MessageChannel, type MessagePort as NodeMessagePort } from "node:worker_threads";

import {
  ChannelClient,
  Emitter,
  VSBuffer,
  decodeFrame,
  encodeControlFrame,
  encodeScopedFrame,
  type AttachmentControl,
  type IChannel,
  type IMessagePassingProtocol,
  type MessagePortPayload,
} from "@mode/rpc";

import { createFrameDecoder, writeFrame } from "./stdioFrames.mts";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../../..");

/** 生产 Host 入口：Rust 壳 spawn 的就是这一个文件（见 docs/specs/tauri-renderer-host.md）。 */
export const HOST_ENTRY = resolve(repoRoot, "packages/desktop/src/host-standalone/index.ts");

/** `host.describe` 的应答形状（跨语言契约只看字段名，与 `HostDescribePayload` 同源）。 */
export interface HostDescribePayload {
  name: string;
  pid: number;
  nodeVersion: string;
  startupId: string;
  dataBaseDir: string;
  dataRoot: string;
  dataBaseDirSource: string;
  rejectedFrames: number;
  serviceChannels: string[];
  /** Task 2：服务面事实（真服务装配 / 派发守卫计数 / stdout 守卫计数）。 */
  serviceSurface?: {
    kind: "services" | "unavailable";
    channelCount: number;
    builtinProviderConfigPath: string;
    builtinProviderConfigSource: string;
    builtinProviderConfigExists: boolean;
    /** `listen` 守卫拦下的订阅次数（未知事件名等）。 */
    guardedSubscriptions: number;
    /** 派发层守卫拦下的同步异常次数（协议层不会再把 Host 带走）。 */
    dispatchGuards: number;
    guardSamples: string[];
    /** 被从 stdout 转走的写入次数（服务日志不该混进帧流）。 */
    stdoutDiverted: number;
    /** 被转走的写入样本（前缀截断）。 */
    stdoutSamples: string[];
  };
  /** Task 2：真实启动序列的终态（映射自真存储/真服务装配）。 */
  startup?: {
    phase: string;
    sequence: number;
    attemptId: string;
    failedPhase?: string;
    errorCode?: string;
    databasePhase?: string;
    history: string[];
  };
  /** Task 2 新增：sidecar 进程内对原生依赖的探测结果（形状同 `host-standalone/nativeDeps.ts`）。 */
  nativeDeps?: {
    sqlite: { ok: boolean; version?: string; roundTrip?: number; error?: string };
    pty: {
      ok: boolean;
      spawned?: boolean;
      exitCode?: number;
      output?: string;
      nativeArtifacts?: string[];
      spawnVerified?: boolean;
      error?: string;
    };
    cua: { injected: boolean; ok: boolean; path?: string; error?: string };
  };
}

export interface ProductionHost {
  child: ChildProcessWithoutNullStreams;
  /** 页面侧端口：与渲染层拿到的是同一种对象（真 `MessagePort`）。 */
  pagePort: NodeMessagePort;
  client: ChannelClient;
  /** 壳观测到「页面 → Host」的帧类型（断言出方向不需要包装）。 */
  clientFrameTypes: string[];
  /**
   * LR-4：为已 attach 的 scope 建一个客户端。帧经页面端口多路复用（base 帧不带前缀），
   * 与页面桥的 scoped port 同一条路径。
   */
  createScopedClient: (scopeId: number) => ChannelClient;
  /** 模拟页面桥发的附件控制帧（scope-ready / scope-detach）。 */
  sendControl: (control: AttachmentControl) => void;
  stderr: () => string;
  stop: () => void;
}

const delayMs = (ms: number): Promise<void> =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

/** RPC 调用一律带超时：挂住必须是**用例失败**，不能变成整轮测试静默卡死。 */
export async function callWithTimeout<T>(
  call: Promise<T>,
  what: string,
  ms = 10_000,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      call,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${what} 超时 ${ms}ms`)), ms);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

/** 起生产 Host 并等它的 `Initialize` 到齐（超时把 stderr 带出来便于定位，收摊不泄漏端口）。 */
export async function connectProductionHost(options: {
  /** 隔离用的「家目录」：同时覆写 HOME 与 USERPROFILE（Windows 认后者、POSIX 认前者）。 */
  home: string;
  /** 隔离用的数据**基**目录；不传时不设 `MODE_DATA_BASE_DIR`，走 HOME 派生。 */
  dataBaseDir?: string;
  /** 壳交给 Host 的启动代 id（默认给测试值）。 */
  startupId?: string;
  /** 额外的子进程环境（例如给 CUA 注入资产路径、打开原生依赖深探测）。 */
  extraEnv?: Record<string, string>;
  env?: NodeJS.ProcessEnv;
}): Promise<ProductionHost> {
  // `extraEnv` 先取成局部量再展开：`...(x ?? {})` 里的空对象回退是多余写法（lint 规则）。
  const extraEnv = options.extraEnv ?? {};
  const env: NodeJS.ProcessEnv = {
    ...(options.env ?? process.env),
    HOME: options.home,
    USERPROFILE: options.home,
    MODE_HOST_STARTUP_ID: options.startupId ?? "task2-test-startup",
    ...extraEnv,
  };
  if (options.dataBaseDir === undefined) {
    delete env.MODE_DATA_BASE_DIR;
  } else {
    env.MODE_DATA_BASE_DIR = options.dataBaseDir;
  }

  const child = spawn("node", ["--import", "tsx", HOST_ENTRY], {
    cwd: repoRoot,
    env,
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderrText = "";
  child.stderr.on("data", (chunk: Buffer) => {
    stderrText += chunk.toString();
  });

  const { port1: pagePort, port2: shellPort } = new MessageChannel();
  const clientFrameTypes: string[] = [];

  // LR-4：页面侧也做与生产桥相同的解复用。base 帧不带前缀；scope 帧按 id 投递。
  const baseMessages = new Emitter<VSBuffer>();
  const scopeMessages = new Map<number, Emitter<VSBuffer>>();
  pagePort.on("message", (data: MessagePortPayload) => {
    if (!(data instanceof Uint8Array)) return;
    const decoded = decodeFrame(data);
    if (!decoded) return;
    if (decoded.scopeId === null) {
      baseMessages.fire(VSBuffer.wrap(decoded.payload));
      return;
    }
    scopeMessages.get(decoded.scopeId)?.fire(VSBuffer.wrap(decoded.payload));
  });
  pagePort.start();

  shellPort.on("message", (data: MessagePortPayload) => {
    clientFrameTypes.push(Object.prototype.toString.call(data));
    if (!(data instanceof Uint8Array)) return; // flow-control 之类的对象不进 RPC 帧
    writeFrame(child.stdin, data);
  });
  shellPort.start();

  child.stdout.on(
    "data",
    createFrameDecoder((frame) => {
      // 生产壳在这里也做同样一层包装（Tauri raw 响应 → Uint8Array）。
      shellPort.postMessage(new Uint8Array(frame));
    }),
  );

  const baseProtocol: IMessagePassingProtocol = {
    send: (buffer) => pagePort.postMessage(buffer.buffer),
    onMessage: baseMessages.event,
  };
  const client = new ChannelClient(baseProtocol);
  const createScopedClient = (scopeId: number): ChannelClient => {
    const emitter = new Emitter<VSBuffer>();
    scopeMessages.set(scopeId, emitter);
    const protocol: IMessagePassingProtocol = {
      send: (buffer) => pagePort.postMessage(encodeScopedFrame(scopeId, buffer.buffer)),
      onMessage: emitter.event,
    };
    return new ChannelClient(protocol);
  };
  const stop = (): void => {
    client.dispose();
    shellPort.close();
    pagePort.close();
    child.kill();
  };
  try {
    await new Promise<void>((resolveInit, rejectInit) => {
      const timer = setTimeout(
        () => rejectInit(new Error(`15s 内没有收到 Host 的 Initialize；stderr:\n${stderrText}`)),
        15_000,
      );
      client.onDidInitialize(() => {
        clearTimeout(timer);
        resolveInit();
      });
    });
  } catch (error) {
    stop();
    throw error;
  }

  // 让 Host 把自己的初始化序列跑完（真服务面 + 真启动），再交还控制权。
  await delayMs(50);
  return {
    child,
    pagePort,
    client,
    clientFrameTypes,
    createScopedClient,
    sendControl: (control) => pagePort.postMessage(encodeControlFrame(control)),
    stderr: () => stderrText,
    stop,
  };
}

/** 拿 `host.describe`（诊断频道，只报事实）。 */
export async function describeHost(host: ProductionHost): Promise<HostDescribePayload> {
  return callWithTimeout(
    host.client.getChannel<IChannel>("host").call<HostDescribePayload>("describe"),
    "host.describe",
  );
}

/**
 * 收摊并**等子进程真的退出**。
 *
 * 为什么不能只 `child.kill()`：真服务面会往隔离目录写 sqlite/WAL/证书，Windows 上文件句柄在进程
 * 退出前不释放，紧接着 `rmSync` 会 EPERM（Task 1 的 Host 不写盘，所以那时看不出这个差别）。
 */
export async function disposeHost(host: ProductionHost, timeoutMs = 10_000): Promise<void> {
  const exited =
    host.child.exitCode !== null || host.child.signalCode !== null
      ? Promise.resolve()
      : new Promise<void>((resolveExit) => {
          const timer = setTimeout(resolveExit, timeoutMs);
          host.child.once("exit", () => {
            clearTimeout(timer);
            resolveExit();
          });
        });
  host.stop();
  await exited;
  // 再给内核一点时间放开文件句柄（Windows 上 taskkill 之后仍可能短暂占用）。
  await delayMs(150);
}

/** 删沙箱：Windows 上手柄释放有延迟，重试几次；最终失败只记录（不把清理失败当成用例失败）。 */
export async function removeSandbox(root: string): Promise<void> {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    try {
      rmSync(root, { recursive: true, force: true });
      return;
    } catch (error) {
      if (attempt === 9) {
        process.stderr.write(`[task2-test] 沙箱清理失败（不影响断言）：${String(error)}\n`);
        return;
      }
      await delayMs(150);
    }
  }
}
