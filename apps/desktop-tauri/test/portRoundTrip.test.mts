/**
 * 覆盖范围：**生产路径的端口往返**（自动化，无需 webview）。
 *
 * 与 `portMessagePath.test.mts` 的分工：那条用例把 Host 换成了 `test/fixtures/stdioChannelHost.mts`
 * 这个替身，只守「端口路径」本身；本用例驱动的是**真的要发布的那份 Host 入口**
 * （`packages/desktop/src/host-standalone/index.ts`，真 `ChannelServer` + stub 服务面），
 * 也就是 M3 Task 1 的交付物本身。它守住四件事：
 *
 * 1. 端口能承载真 Host 的 `Initialize` 与一次真 RPC 往返（`host.describe`，返回值里带 pid /
 *    startupId / dataRoot，能证明应答确实来自那个子进程、且读的是我们给它的环境）；
 * 2. **stub 服务面对每个方法都显式 `not_implemented`**（DR-T1-1：不许返回 `undefined` 或空对象，
 *    否则调用方会把「没实现」读成「成功但没数据」）——遍历 Host 自报的服务频道逐个断言；
 * 3. **Host 入口自己校验帧形状**：非 RPC 帧必须被丢弃而不是把 `ChannelServer` 带崩
 *    （spike 文档 §4.6 的实测崩溃：非 RPC 帧 → `TypeError: Cannot read properties of undefined`）；
 * 4. **数据隔离生效**：`MODE_DATA_BASE_DIR` 与 `HOME`/`USERPROFILE` 两条覆写都能改变 Host 认到的
 *    数据根，且运行完**不会**在隔离目录里真造出 `.mode`（Task 1 的 Host 不写盘）。
 *
 * 真 webview 那一半（`initialization_script` 注入、`window.postMessage` + transfer、
 * `event.source === window`、ACL、投递时序）CI 跑不了：由
 * `test/webviewRoundTrip.test.mts`（opt-in，默认 skip）覆盖。
 */
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { MessageChannel, type MessagePort as NodeMessagePort } from "node:worker_threads";
import assert from "node:assert/strict";
import test from "node:test";

import {
  ChannelClient,
  MessagePortProtocol,
  type IChannel,
  type MessagePortLike,
  type MessagePortPayload,
} from "@mode/rpc";

import { createFrameDecoder, writeFrame } from "./fixtures/stdioFrames.mts";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
/** 生产 Host 入口：Rust 壳 spawn 的就是这一个文件（见 docs/specs/tauri-renderer-host.md）。 */
const hostEntry = resolve(repoRoot, "packages/desktop/src/host-standalone/index.ts");

const STARTUP_ID = "port-round-trip-startup-1";

interface HostUnderTest {
  child: ChildProcessWithoutNullStreams;
  /** 页面侧端口：与渲染层拿到的是同一种对象（真 `MessagePort`）。 */
  pagePort: NodeMessagePort;
  /** 壳观测到「页面 → Host」的帧类型，用于断言出方向不需要包装。 */
  clientFrameTypes: string[];
  stderr: () => string;
  stop: () => void;
}

/** 把 Node 的 `MessagePort` 包成 `packages/rpc` 要求的 `MessagePortLike`（形状见 protocol.ts）。 */
function asMessagePortLike(port: NodeMessagePort): MessagePortLike {
  return {
    addEventListener(_type: "message", listener: (event: { data: MessagePortPayload }) => void) {
      port.on("message", (data: MessagePortPayload) => listener({ data }));
    },
    removeEventListener(_type: "message", listener: (event: { data: MessagePortPayload }) => void) {
      port.off("message", listener as unknown as (value: MessagePortPayload) => void);
    },
    postMessage(message: MessagePortPayload) {
      port.postMessage(message);
    },
    start() {
      port.start();
    },
    close() {
      port.close();
    },
  };
}

/**
 * 起真 Host，并把「页面 port2 ↔ Host stdio」按生产口径对接：
 * 出方向原样写帧（客户端帧本来就是 `Uint8Array`），入方向**包成 `Uint8Array`** 再投端口
 * （Tauri 的 raw 响应是 `ArrayBuffer`，不包会被 `MessagePortProtocol` 静默丢弃，spike §4.3）。
 */
function startHost(options: {
  /** 隔离用的数据根；不传时不设 `MODE_DATA_BASE_DIR`，走 HOME 派生。 */
  dataBaseDir?: string;
  /** 隔离用的「家目录」：同时覆写 HOME 与 USERPROFILE（Windows 认后者、POSIX 认前者）。 */
  home: string;
}): HostUnderTest {
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    HOME: options.home,
    USERPROFILE: options.home,
    MODE_HOST_STARTUP_ID: STARTUP_ID,
  };
  if (options.dataBaseDir === undefined) {
    delete env.MODE_DATA_BASE_DIR;
  } else {
    env.MODE_DATA_BASE_DIR = options.dataBaseDir;
  }

  const child = spawn("node", ["--import", "tsx", hostEntry], {
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

  return {
    child,
    pagePort,
    clientFrameTypes,
    stderr: () => stderrText,
    stop: () => {
      shellPort.close();
      pagePort.close();
      child.kill();
    },
  };
}

interface HostDescribe {
  name: string;
  pid: number;
  nodeVersion: string;
  startupId: string;
  dataRoot: string;
  dataRootSource: string;
  serviceChannels: string[];
}

const delay = (ms: number): Promise<void> =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

/** RPC 调用一律带超时：挂住必须是**用例失败**，不能变成整轮测试静默卡死。 */
async function callWithTimeout<T>(call: Promise<T>, what: string, ms = 10_000): Promise<T> {
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

/** 等 Host 的 `Initialize` 到达并把 `ChannelClient` 建好（超时把 stderr 带出来便于定位）。 */
async function connect(options: { dataBaseDir?: string; home: string }): Promise<{
  host: HostUnderTest;
  client: ChannelClient;
}> {
  const host = startHost(options);
  const client = new ChannelClient(new MessagePortProtocol(asMessagePortLike(host.pagePort)));
  try {
    await new Promise<void>((resolveInit, rejectInit) => {
      const timer = setTimeout(
        () => rejectInit(new Error(`15s 内没有收到 Host 的 Initialize；stderr:\n${host.stderr()}`)),
        15_000,
      );
      client.onDidInitialize(() => {
        clearTimeout(timer);
        resolveInit();
      });
    });
  } catch (error) {
    // 连接失败时也要收摊：泄漏的 MessagePort 会把事件循环挂住（Node 测试跑不完）。
    client.dispose();
    host.stop();
    throw error;
  }
  return { host, client };
}

test("生产 Host sidecar：Initialize 与一次真 RPC 往返可以走端口", async () => {
  const sandbox = mkdtempSync(join(tmpdir(), "mode-host-roundtrip-"));
  const home = join(sandbox, "home");
  const dataBaseDir = join(sandbox, "data");
  const { host, client } = await connect({ dataBaseDir, home });
  try {
    const describe = await callWithTimeout(
      client.getChannel<IChannel>("host").call<HostDescribe>("describe"),
      "host.describe",
    );

    assert.equal(describe.name, "mode-host-sidecar");
    assert.ok(describe.pid > 0, "返回值来自子进程");
    assert.match(describe.nodeVersion, /^v\d+\./);
    assert.equal(describe.startupId, STARTUP_ID, "Host 认到的 startupId 来自壳给的环境变量");
    assert.equal(describe.dataRoot, dataBaseDir, "MODE_DATA_BASE_DIR 覆写生效");
    assert.equal(describe.dataRootSource, "MODE_DATA_BASE_DIR");
    assert.ok(
      describe.serviceChannels.length >= 40,
      `stub 服务面覆盖全部服务频道，实际 ${describe.serviceChannels.length} 个`,
    );
    assert.ok(describe.serviceChannels.includes("file"), "服务频道名与 ServiceChannels 一致");
    assert.deepEqual(
      host.clientFrameTypes,
      ["[object Uint8Array]"],
      "出方向的客户端帧本来就是 Uint8Array，壳侧不需要为它做包装",
    );
  } finally {
    client.dispose();
    host.stop();
    rmSync(sandbox, { recursive: true, force: true });
  }
});

test("stub 服务面对每个命令都显式 not_implemented（不返回 undefined / 空对象）", async () => {
  const sandbox = mkdtempSync(join(tmpdir(), "mode-host-stub-"));
  const { host, client } = await connect({ home: join(sandbox, "home") });
  try {
    const describe = await callWithTimeout(
      client.getChannel<IChannel>("host").call<HostDescribe>("describe"),
      "host.describe",
    );
    assert.ok(describe.serviceChannels.length > 0);

    for (const channelName of describe.serviceChannels) {
      await assert.rejects(
        client.getChannel<IChannel>(channelName).call("__m3_task1_probe__"),
        (error: unknown) => {
          assert.ok(error instanceof Error, `${channelName}: 失败必须是 Error，而不是返回值`);
          assert.match(error.message, /not_implemented/);
          assert.ok(
            error.message.includes(channelName) && error.message.includes("__m3_task1_probe__"),
            `${channelName}: 错误必须点名频道与命令，便于定位；实际：${error.message}`,
          );
          return true;
        },
      );
    }
  } finally {
    client.dispose();
    host.stop();
    rmSync(sandbox, { recursive: true, force: true });
  }
});

test("Host 入口拒绝形状不合法的帧，不会把 ChannelServer 带崩", async () => {
  const sandbox = mkdtempSync(join(tmpdir(), "mode-host-garbage-"));
  const { host, client } = await connect({ home: join(sandbox, "home") });
  try {
    // ① 空帧（spike §4.6 的崩溃形态：协议层拿到 undefined 的 header）；
    // ② 形状合法但方向错的 envelope：`[200]` = `ResponseType.Initialize`，只应出现在
    //    Host → 页面方向；字节取自实测的一次真 Initialize 帧（`[4, 1, 6, 200]`）。
    writeFrame(host.child.stdin, new Uint8Array(0));
    writeFrame(host.child.stdin, Uint8Array.from([4, 1, 6, 200]));
    await delay(300);

    assert.equal(host.child.exitCode, null, "Host 进程仍然活着");
    const describe = await callWithTimeout(
      client.getChannel<IChannel>("host").call<HostDescribe>("describe"),
      "host.describe",
    );
    assert.equal(describe.name, "mode-host-sidecar", "被丢弃的坏帧不影响后续合法 RPC");
    assert.match(host.stderr(), /dropped|rejected/i, "坏帧必须留下可诊断的痕迹");
  } finally {
    client.dispose();
    host.stop();
    rmSync(sandbox, { recursive: true, force: true });
  }
});

test("数据隔离：两条覆写都生效，且运行后不产生真实数据目录", async () => {
  // ① MODE_DATA_BASE_DIR 优先；
  const sandboxEnv = mkdtempSync(join(tmpdir(), "mode-host-isolation-env-"));
  const envHome = join(sandboxEnv, "home");
  const envDataRoot = join(sandboxEnv, "data");
  const caseA = await connect({ dataBaseDir: envDataRoot, home: envHome });
  try {
    const describe = await callWithTimeout(
      caseA.client.getChannel<IChannel>("host").call<HostDescribe>("describe"),
      "host.describe(caseA)",
    );
    assert.equal(describe.dataRoot, envDataRoot);
    assert.equal(describe.dataRootSource, "MODE_DATA_BASE_DIR");
  } finally {
    caseA.client.dispose();
    caseA.host.stop();
  }

  // ② 没有 MODE_DATA_BASE_DIR 时由 HOME/USERPROFILE 派生 `<home>/.mode`；
  const sandboxHome = mkdtempSync(join(tmpdir(), "mode-host-isolation-home-"));
  const onlyHome = join(sandboxHome, "home");
  const caseB = await connect({ home: onlyHome });
  try {
    const describe = await callWithTimeout(
      caseB.client.getChannel<IChannel>("host").call<HostDescribe>("describe"),
      "host.describe(caseB)",
    );
    assert.equal(describe.dataRoot, join(onlyHome, ".mode"));
    assert.equal(describe.dataRootSource, "home");
  } finally {
    caseB.client.dispose();
    caseB.host.stop();
  }

  await delay(200); // 给「万一有写入」留下浮出时间，再断言没有落盘
  assert.equal(
    existsSync(join(onlyHome, ".mode")),
    false,
    "Task 1 的 Host 不写盘：隔离目录里不得出现 .mode（更不得碰用户真实数据根）",
  );
  assert.equal(existsSync(envDataRoot), false, "MODE_DATA_BASE_DIR 分支同样不得造目录");

  rmSync(sandboxEnv, { recursive: true, force: true });
  rmSync(sandboxHome, { recursive: true, force: true });
});
