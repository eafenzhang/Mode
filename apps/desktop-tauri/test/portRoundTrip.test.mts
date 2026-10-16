/**
 * 覆盖范围：**生产路径的端口往返**（自动化，无需 webview）。
 *
 * 与 `portMessagePath.test.mts` 的分工：那条用例把 Host 换成了 `test/fixtures/stdioChannelHost.mts`
 * 这个替身，只守「端口路径」本身；本用例驱动的是**真的要发布的那份 Host 入口**
 * （`packages/desktop/src/host-standalone/index.ts`，真 `ChannelServer` + Task 2 的真服务面），
 * 也就是 M3 Task 1 交付、Task 2 换心之后的产品入口本身。它守住五件事：
 *
 * 1. 端口能承载真 Host 的 `Initialize` 与一次真 RPC 往返（`host.describe`，返回值里带 pid /
 *    startupId / dataRoot，能证明应答确实来自那个子进程、且读的是我们给它的环境）；
 * 2. **服务面对未知命令显式报错**（Task 1 的 stub 是 `not_implemented`，Task 2 换成
 *    `ProxyChannel` 的 `Method not found`）：规则不变——**绝不**返回 `undefined` 或空对象，
 *    否则调用方会把「没实现」读成「成功但没数据」（DR-T1-1）。同时**至少一条真命令要返回真值**，
 *    否则「服务面换成了真的」这件事没被证明（真服务面会对每条命令都报错的形态是允许存在的）。
 * 3. **订阅未知事件不杀 Host、也不带走同一条传输上的在途 RPC**（Task 1 的 C1 换了主体：
 *    当时抛的是我们的 stub，现在是 `ProxyChannel.fromService`，而
 *    `ChannelServer.onEventListen` 至今没有 try/catch）；
 * 4. **Host 入口自己校验帧形状**：非 RPC 帧必须被丢弃而不是把 `ChannelServer` 带崩
 *    （spike 文档 §4.6 的实测崩溃：非 RPC 帧 → `TypeError: Cannot read properties of undefined`）；
 * 5. **数据隔离是真的**（Task 2 起 Host 真写盘）：两条覆写各自生效、数据根按仓库既有口径取
 *    `{base}/.mode`（`packages/services/src/paths.ts` 的 `getModeDataRootDir`）、真写入落在
 *    隔离目录里，且**用户真实的 `~/.mode` 一字未动**。`HOME`/`USERPROFILE` 由用例注入的环境
 *    继承到子进程（壳不显式覆写），所以两条覆写都必须断言。
 *
 * 真 webview 那一半（`initialization_script` 注入、`window.postMessage` + transfer、
 * `event.source === window`、ACL、投递时序）CI 跑不了：由
 * `test/webviewRoundTrip.test.mts`（opt-in，默认 skip）覆盖。
 */
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
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
  /** 数据**基**目录：`MODE_DATA_BASE_DIR` 的语义（不是数据根）。 */
  dataBaseDir: string;
  /** 数据根 = `{dataBaseDir}/.mode`（与 packages/services/src/paths.ts 同口径）。 */
  dataRoot: string;
  dataBaseDirSource: string;
  serviceChannels: string[];
  /** Task 2：服务面事实（真服务装配 / 派发守卫计数）。 */
  serviceSurface?: {
    kind?: string;
    channelCount?: number;
    guardedSubscriptions?: number;
    dispatchGuards?: number;
    guardSamples?: string[];
  };
}

const delay = (ms: number): Promise<void> =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

/**
 * 删隔离沙箱：真服务会往里面写 sqlite/WAL/设置文件，Windows 上文件句柄在进程退出前不释放，
 * 紧接着 `rmSync` 会 `EPERM`（Task 1 的 Host 不写盘，所以那时看不出这个差别）。重试几次；
 * 最终失败只留痕，不把清理失败当成用例失败（断言已经跑完了）。
 */
async function removeSandbox(root: string): Promise<void> {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    try {
      rmSync(root, { recursive: true, force: true });
      return;
    } catch (error) {
      if (attempt === 9) {
        process.stderr.write(`[port-round-trip] 沙箱清理失败（不影响断言）：${String(error)}\n`);
        return;
      }
      await delay(150);
    }
  }
}

/** 列目录条目（排序后）；目录不存在或读不了时返回 `null`——「不存在」也是一种要被断言的事实。 */
function listEntries(root: string): string[] | null {
  try {
    return readdirSync(root).sort();
  } catch {
    return null;
  }
}

/** 收摊并**等子进程真的退出**（否则下一次 spawn 会撞上还没放开的文件句柄）。 */
async function stopHostAndWait(host: HostUnderTest, client: ChannelClient): Promise<void> {
  const exited =
    host.child.exitCode !== null || host.child.signalCode !== null
      ? Promise.resolve()
      : new Promise<void>((resolveExit) => {
          const timer = setTimeout(resolveExit, 10_000);
          host.child.once("exit", () => {
            clearTimeout(timer);
            resolveExit();
          });
        });
  client.dispose();
  host.stop();
  await exited;
  await delay(150); // 给内核一点时间放开文件句柄（Windows 上 taskkill 之后仍可能短暂占用）
}

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
    assert.equal(describe.dataBaseDir, dataBaseDir, "MODE_DATA_BASE_DIR 覆写的是**基**目录");
    assert.equal(
      describe.dataRoot,
      join(dataBaseDir, ".mode"),
      "数据根 = {base}/.mode（口径同 packages/services/src/paths.ts 的 getModeDataRootDir）",
    );
    assert.equal(describe.dataBaseDirSource, "MODE_DATA_BASE_DIR");
    assert.ok(
      describe.serviceChannels.length >= 40,
      `真服务面覆盖全部服务频道，实际 ${describe.serviceChannels.length} 个`,
    );
    assert.ok(describe.serviceChannels.includes("file"), "服务频道名与 ServiceChannels 一致");
    assert.deepEqual(
      host.clientFrameTypes,
      ["[object Uint8Array]"],
      "出方向的客户端帧本来就是 Uint8Array，壳侧不需要为它做包装",
    );
  } finally {
    await stopHostAndWait(host, client);
    await removeSandbox(sandbox);
  }
});

test("真服务面对未知命令显式报错（不返回 undefined / 空对象），stub 的 not_implemented 已消失", async () => {
  const sandbox = mkdtempSync(join(tmpdir(), "mode-host-unknown-"));
  const { host, client } = await connect({ home: join(sandbox, "home") });
  try {
    const describe = await callWithTimeout(
      client.getChannel<IChannel>("host").call<HostDescribe>("describe"),
      "host.describe",
    );
    assert.ok(describe.serviceChannels.length > 0);
    assert.equal(
      describe.serviceSurface?.kind,
      "services",
      `Host 必须自报装的是真服务面（不是 stub）：${JSON.stringify(describe.serviceSurface)}`,
    );

    // ① 真命令返回真值。stub 对**任何**命令都会 `not_implemented`，所以这一条本身就否掉了 stub
    //    （另外两条是它的反证：未知命令必须报「方法不存在」而不是「还没实现」）。
    const info = await callWithTimeout(
      client.getChannel<IChannel>("system").call<{ platform: string }>("info"),
      "system.info",
    );
    assert.equal(info.platform, process.platform, "system.info() 返回本进程平台的真实值");

    // ② 未知命令必须是**错误**，不是 `undefined` / 空对象 —— DR-T1-1 的规则在真服务面上的形态。
    for (const channelName of describe.serviceChannels) {
      await assert.rejects(
        client.getChannel<IChannel>(channelName).call("__m3_task2_probe__"),
        (error: unknown) => {
          assert.ok(error instanceof Error, `${channelName}: 失败必须是 Error，而不是返回值`);
          assert.match(
            error.message,
            /Method not found/,
            `${channelName}: 真服务面应报方法不存在；实际：${error.message}`,
          );
          assert.ok(
            error.message.includes("__m3_task2_probe__"),
            `${channelName}: 错误必须点名命令，便于定位；实际：${error.message}`,
          );
          assert.doesNotMatch(
            error.message,
            /not_implemented/,
            `${channelName}: stub 的标记不得再现（服务面被换回 stub 就是这条）`,
          );
          return true;
        },
      );
    }
  } finally {
    await stopHostAndWait(host, client);
    await removeSandbox(sandbox);
  }
});

test("订阅未知事件不杀 Host，也不带走同一条传输上的在途 RPC", async () => {
  // Task 1 的 C1 在这里**换了主体**：当时抛错的是我们的 stub，现在是 `ProxyChannel.fromService`
  // ——真服务面的 `listen` 对未知事件名直接抛 `Event not found`，而
  // `ChannelServer.onEventListen` **没有** try/catch，于是「一次坏订阅 = Host 进程当场死亡 +
  // 同一端口上的在途 RPC 全部变 transport_closed」。渲染层一挂载就会订阅服务事件，
  // 所以守卫必须装在唯一的入口上（`registerChannel` + 派发入口），不是逐个服务打补丁。
  const sandbox = mkdtempSync(join(tmpdir(), "mode-host-listen-guard-"));
  const { host, client } = await connect({ home: join(sandbox, "home") });
  try {
    const describe = await callWithTimeout(
      client.getChannel<IChannel>("host").call<HostDescribe>("describe"),
      "host.describe",
    );
    // `setting` 的实现只有方法、没有任何 `on*` 事件 → 任何事件名都必然走「未知事件」分支。
    const channelName = describe.serviceChannels.includes("setting")
      ? "setting"
      : describe.serviceChannels[0];
    assert.ok(channelName, "Host 应至少暴露一个服务频道");

    const received: unknown[] = [];
    // ① 与坏订阅**并发**发一个真 RPC：C1 的实际杀伤是「在途 RPC 一起死」，不只是「订阅失败」。
    const inFlight = callWithTimeout(
      client.getChannel<IChannel>("host").call<HostDescribe>("describe"),
      "host.describe(与坏订阅并发)",
    );
    const subscription = client
      .getChannel<IChannel>(channelName)
      .listen("onDidChange")((value: unknown) => received.push(value));
    const during = await inFlight;
    await delay(300);

    // ② Host 还活着，且同一条传输上的在途 RPC 照常完成（本用例存在的首要理由）。
    assert.equal(
      host.child.exitCode,
      null,
      `订阅未知事件后 Host 必须还活着；stderr:\n${host.stderr()}`,
    );
    assert.equal(during.name, "mode-host-sidecar", "在途 RPC 不得因坏订阅变成 transport_closed");
    // ③ 不假装有事件：一个都没投（不给「订阅成功」的假象）。
    assert.deepEqual(received, [], "守卫不投假事件");
    // ④ 也不静默：stderr 留痕，且能从 `host.describe` 观察到守卫计数。
    assert.match(host.stderr(), /Event not found|listen_guard/, "拒绝必须留痕（stderr）");
    const after = await callWithTimeout(
      client.getChannel<IChannel>("host").call<HostDescribe>("describe"),
      "host.describe(订阅之后)",
    );
    assert.ok(
      (after.serviceSurface?.guardedSubscriptions ?? 0) >= 1,
      `host.describe 应能观察到 guardedSubscriptions，实际 ${JSON.stringify(after.serviceSurface)}`,
    );
    subscription.dispose();
  } finally {
    await stopHostAndWait(host, client);
    await removeSandbox(sandbox);
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
    await stopHostAndWait(host, client);
    await removeSandbox(sandbox);
  }
});

test("数据隔离是真的：两条覆写各自生效，真写入落在隔离根，用户真实 ~/.mode 未被触碰", async () => {
  // Task 1 的 Host 不写盘，所以当时的断言是「隔离目录里连目录都不出现」。Task 2 换上真服务后
  // 那条断言变成**假断言**（真服务会写 sqlite/WAL/设置文件，它必然为 false），但被守的事实
  // 不能因此丢掉——它升级成更强的形态：**写入确实发生了**（否则断言是空的）、写入落在
  // 隔离根里、且用户真实的数据根一字未动。
  const realRoot = join(homedir(), ".mode");
  const realEntriesBefore = listEntries(realRoot);
  const realExistedBefore = realEntriesBefore !== null;

  // ① MODE_DATA_BASE_DIR 优先；
  const sandboxEnv = mkdtempSync(join(tmpdir(), "mode-host-isolation-env-"));
  const envHome = join(sandboxEnv, "home");
  const envBaseDir = join(sandboxEnv, "data");
  const caseA = await connect({ dataBaseDir: envBaseDir, home: envHome });
  try {
    const describe = await callWithTimeout(
      caseA.client.getChannel<IChannel>("host").call<HostDescribe>("describe"),
      "host.describe(caseA)",
    );
    assert.equal(describe.dataBaseDir, envBaseDir, "覆写的是基目录");
    assert.equal(
      describe.dataRoot,
      join(envBaseDir, ".mode"),
      "根是基目录下的 .mode —— 不是把基目录当根（2026-10-16 复评钉的 I2）",
    );
    assert.equal(describe.dataBaseDirSource, "MODE_DATA_BASE_DIR");
  } finally {
    await stopHostAndWait(caseA.host, caseA.client);
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
    assert.equal(describe.dataBaseDir, onlyHome, "没有覆写时基目录就是家目录");
    assert.equal(describe.dataRoot, join(onlyHome, ".mode"));
    assert.equal(describe.dataBaseDirSource, "home");
  } finally {
    await stopHostAndWait(caseB.host, caseB.client);
  }

  // ③ 真服务真写盘：两个分支都必须落在各自的隔离根里（这条同时证明前面的隔离断言不是空的）。
  assert.equal(
    existsSync(join(envBaseDir, ".mode")),
    true,
    `真服务的写入必须落在 {base}/.mode；实际 ${envBaseDir} 下：` +
      (existsSync(envBaseDir) ? readdirSync(envBaseDir).join(",") : "(基目录都不存在)"),
  );
  assert.equal(
    existsSync(join(onlyHome, ".mode")),
    true,
    "没有 MODE_DATA_BASE_DIR 时写入必须落在 <home>/.mode（HOME/USERPROFILE 覆写生效）",
  );
  assert.equal(
    existsSync(join(envBaseDir, ".mode", ".mode")),
    false,
    "数据根绝不能是 {base}/.mode/.mode（那把基目录当根用的口径错误）",
  );
  assert.equal(existsSync(join(onlyHome, ".mode", ".mode")), false, "同上（HOME 分支）");

  // ④ 用户真实数据根：比对**存在性与条目集合**，不比对目录 mtime——mtime 会被别的进程对真实
  //    根的写入（它会正常发生）改变，拿它当判据会假红；条目集合只在「真的有人往里加/删东西」时变。
  const realEntriesAfter = listEntries(realRoot);
  assert.equal(
    realEntriesAfter !== null,
    realExistedBefore,
    `本次运行不得创建或删除用户真实数据根 ${realRoot}`,
  );
  if (realEntriesBefore && realEntriesAfter) {
    assert.deepEqual(
      realEntriesAfter,
      realEntriesBefore,
      `用户真实数据根的条目集合不得被本次运行改变（${realRoot}）`,
    );
  }

  await removeSandbox(sandboxEnv);
  await removeSandbox(sandboxHome);
});
