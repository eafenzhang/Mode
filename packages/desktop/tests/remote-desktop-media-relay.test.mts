/**
 * host 媒体中继回归：本地管道长度前缀分帧、kind 路由、media WS 上行（ready/ping/输入）、
 * 背压丢弃与超长帧防御。运行：
 * pnpm exec tsx --test packages/desktop/tests/*.test.mjs packages/desktop/tests/*.test.mts
 * 扩展名 .mts：语言政策新测试禁 .mjs（docs/specs/language-policy.md）。
 * 入口走 tsx（2026-10-15 起）：本文件经中继间接 import 了带 `.js` 后缀的本地模块，而
 * `node --test` 的原生类型剥离不做 .js → .ts 重写，会 ERR_MODULE_NOT_FOUND。
 * 每个用例用 try/finally 收口：断言失败时也必须关掉回环连接，否则进程句柄挂住不退。
 *
 * server 级收口场景（bye 关中继回归）经 RD_MEDIA_RELAY_SERVER_CASE=1 的 tsx 子进程执行：
 * remoteDesktopServer 的 .js 后缀与 @mode 别名只有 tsx 能解析。
 */
import assert from "node:assert/strict";
import test from "node:test";
import { connect, createServer, type Socket } from "node:net";
import { once } from "node:events";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { createRemoteDesktopMediaRelay, type WsLike } from "../src/host/remoteDesktopMediaRelay.ts";

/** 子进程模式开关：值为 "1" 时本文件只跑 server 收口场景，不注册 relay 用例。 */
const SERVER_CASE_ENV = "RD_MEDIA_RELAY_SERVER_CASE";

/** 管道帧：[u32 len 大端][u8 kind][payload]，len = 1 + payload.length（契约 B）。 */
function pipeFrame(kind: number, payload: Uint8Array): Buffer {
  const body = Buffer.concat([Buffer.from([kind]), Buffer.from(payload)]);
  const header = Buffer.alloc(4);
  header.writeUInt32BE(body.length, 0);
  return Buffer.concat([header, body]);
}

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

const noopLogger = { info(): void {}, warn(): void {} };

/** 只收集帧的 fake media WS（WsLike 最小鸭子类型；bufferedAmount 由 buffered.amount 可变模拟）。 */
function makeFakeWs(): {
  ws: WsLike;
  texts: string[];
  binaries: Buffer[];
  buffered: { amount: number };
} {
  const texts: string[] = [];
  const binaries: Buffer[] = [];
  const buffered = { amount: 0 };
  const ws: WsLike = {
    send: (data) => {
      if (typeof data === "string") {
        texts.push(data);
      } else {
        binaries.push(Buffer.from(data));
      }
    },
    get bufferedAmount(): number {
      return buffered.amount;
    },
  };
  return { ws, texts, binaries, buffered };
}

/** 无论断言成败都关中继与 helper 连接，保证 node --test 进程能退出。 */
function cleanup(relay: { close(): void }, helper?: Socket): void {
  relay.close();
  helper?.destroy();
}

/** 轮询等待条件成立（5s 上限），用于异步回帖/日志这类无事件可 await 的时序点。 */
async function waitFor(predicate: () => boolean, what: string): Promise<void> {
  const deadline = Date.now() + 5_000;
  while (!predicate()) {
    if (Date.now() > deadline) {
      assert.fail(`等待超时：${what}`);
    }
    await sleep(10);
  }
}

/** 可记录 on 注册与 close 调用的 fake socket（server 级场景用，多 handler 按事件累积）。 */
function makeFakeSocket(): {
  on(event: string, cb: (...args: never[]) => void): void;
  send(data: unknown): void;
  close(code?: number, reason?: string): void;
  fire(event: string, ...args: unknown[]): void;
  closes: Array<{ code?: number; reason?: string }>;
} {
  const handlers = new Map<string, Array<(...args: unknown[]) => void>>();
  const closes: Array<{ code?: number; reason?: string }> = [];
  return {
    on(event, cb) {
      const list = handlers.get(event) ?? [];
      list.push(cb as (...args: unknown[]) => void);
      handlers.set(event, list);
    },
    send() {},
    close(code, reason) {
      closes.push({ code, reason });
      for (const cb of handlers.get("close") ?? []) {
        cb(code, reason);
      }
    },
    fire(event, ...args) {
      for (const cb of handlers.get(event) ?? []) {
        cb(...args);
      }
    },
    closes,
  };
}

/** 从（基线后的）info 日志里取最后一次记录的中继端口。 */
function portFromLines(lines: string[]): number | null {
  let port: number | null = null;
  for (const line of lines) {
    const match = /media relay listening on 127\.0\.0\.1:(\d+)/u.exec(line);
    if (match) {
      port = Number(match[1]);
    }
  }
  return port;
}

/** 断言端口已释放：可重新 bind（带少量重试，容忍 close 的内核侧时序）。 */
async function assertPortFree(port: number): Promise<void> {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const probe = createServer();
    try {
      await new Promise<void>((resolve, reject) => {
        probe.once("error", reject);
        probe.listen({ port, host: "127.0.0.1" }, resolve);
      });
      probe.close();
      return;
    } catch {
      await sleep(25);
    }
  }
  assert.fail(`中继口 ${port} 未释放`);
}

/**
 * server 级收口场景（fix round 1 的 Important 回归）：
 * peer bye 走 closeActiveSocket → 必须关中继（口/helper 连接/媒体 WS 三样全收口），
 * 下一会话重新 listen 不能被残留 helper「第二条连接拒绝」堵死。
 */
async function runSessionByeRelayCloseCase(): Promise<void> {
  // 剔除环境 kill-switch，保证准入只看被控开关（下面 mock 恒开）。
  delete process.env.MODE_REMOTE_DESKTOP_KILL_SWITCH;
  const { createRemoteDesktopHostServer } = await import("../src/host/remoteDesktopServer.js");
  const { RemoteDesktopHostMessageTypes } = await import("../src/host/remoteDesktopMessages.js");

  const infoLines: string[] = [];
  const posted: Array<{ type?: string }> = [];
  const host = createRemoteDesktopHostServer({
    credentials: {
      load: async (key: string) => (key === "remote-desktop:server-enabled" ? "1" : "[]"),
      save: async () => {},
      delete: async () => {},
      list: async () => [],
    },
    logger: {
      info: (...args: unknown[]) => infoLines.push(args.map(String).join(" ")),
      warn: () => {},
    },
    postToMain: (message) => {
      posted.push(message);
    },
    // 用活 handle 走 readClients 的内存路径，绕开凭据文件读取。
    getActiveLanAccessServer: () =>
      ({
        getState: () => ({
          clients: [{ id: "client-a", label: "Office-PC", remoteDesktopAuthorized: true }],
        }),
        setClientRemoteDesktopAuthorized: async () => {},
      }) as never,
  });
  await host.loadServerState();

  /** 同意通过 → 等中继口日志；返回端口。基线隔离保证「等的是本轮的新事件」。 */
  const startSession = async (): Promise<number> => {
    const postedBaseline = posted.length;
    const infoBaseline = infoLines.length;
    const request = host.hooks.requestSession("client-a");
    await waitFor(
      () =>
        posted
          .slice(postedBaseline)
          .some(
            (message) => message.type === RemoteDesktopHostMessageTypes.RemoteDesktopConsentRequest,
          ),
      "同意请求发出",
    );
    host.handleMessage({
      type: RemoteDesktopHostMessageTypes.RemoteDesktopConsentResult,
      clientId: "client-a",
      decision: "allow",
      remember: false,
    });
    assert.deepEqual(await request, { ok: true });
    const newInfo = () => infoLines.slice(infoBaseline);
    await waitFor(
      () => newInfo().some((line) => line.includes("media relay listening")),
      "中继口开始监听",
    );
    const port = portFromLines(newInfo());
    assert.ok(port !== null, "应记录中继端口");
    return port;
  };

  // ── 会话 A：同意 → 中继口 → helper 连入（跨越 bye 存活是本回归的前提）→ 信令/媒体 WS 挂接 → bye ──
  const portA = await startSession();
  const helperA = connect({ host: "127.0.0.1", port: portA });
  await once(helperA, "connect");
  const sessionSocket = makeFakeSocket();
  host.hooks.onSessionSocket("client-a", sessionSocket);
  const mediaSocket = makeFakeSocket();
  host.hooks.onMediaSocket("client-a", mediaSocket);

  // peer bye（信令 socket 的 message 事件）→ closeActiveSocket 路径
  sessionSocket.fire("message", Buffer.from(JSON.stringify({ type: "bye" })));

  // ① 媒体 WS 被 relay 以 1000 收口（修复前 closeActiveSocket 不关中继 → 此处为空）
  assert.deepEqual(
    mediaSocket.closes.filter((entry) => entry.code === 1000),
    [{ code: 1000, reason: "session closed" }],
    "bye 后媒体 WS 应被 relay 以 1000 关闭",
  );
  // ② helper 管道连接被拆（残留连接会在下一会话触发「第二条连接拒绝」）
  await Promise.race([once(helperA, "close"), sleep(1000)]);
  assert.equal(helperA.destroyed, true, "bye 后 helper 管道连接应被关闭");
  // ③ 中继口释放（可重绑）
  await assertPortFree(portA);
  // ④ 会话收口已回帖
  assert.ok(
    posted.some(
      (message) => message.type === RemoteDesktopHostMessageTypes.RemoteDesktopSessionClosed,
    ),
    "bye 应回帖会话关闭",
  );

  // ── 会话 B：再次同意 → 中继重开 → 新 helper 连入不被双连接拒绝堵死 ──
  const portB = await startSession();
  const helperB = connect({ host: "127.0.0.1", port: portB });
  await once(helperB, "connect");
  await sleep(60); // 若有残留连接触发拒绝，服务端会立刻拆掉新连接
  assert.equal(helperB.destroyed, false, "下一会话的 helper 不应被「第二条连接拒绝」堵死");

  helperB.destroy();
  host.dispose();
}

if (process.env[SERVER_CASE_ENV] === "1") {
  await runSessionByeRelayCloseCase();
  console.log("RD_MEDIA_RELAY_SERVER_CASE_OK");
} else {
  test("粘包/半包正确分帧；kind0 透传出口、kind1 触发 onStatus", async () => {
    const seen: unknown[] = [];
    const relay = createRemoteDesktopMediaRelay({
      logger: noopLogger,
      onFrame: (p) => seen.push(["frame", Buffer.from(p).toString("hex")]),
      onStatus: (frozen, reason) => seen.push(["status", frozen, reason]),
      onReady: () => seen.push(["ready"]),
      onInput: (e) => seen.push(["input", e]),
    });
    const helper = connect({ host: "127.0.0.1", port: await relay.listen() });
    try {
      await once(helper, "connect");
      // payload 契约 B 形状：seq=7(00000007)、flags=1、密文两字节 → 7 字节原样透传
      const first = pipeFrame(0, Buffer.from([0, 0, 0, 7, 1, 9, 9]));
      const second = pipeFrame(1, Buffer.from(JSON.stringify({ frozen: true, reason: "locked" })));
      // 半包：先写一半第一个帧
      helper.write(first.subarray(0, 3));
      await sleep(10);
      helper.write(Buffer.concat([first.subarray(3), second]));
      await sleep(30);
      assert.deepEqual(seen, [
        ["frame", "00000007010909"],
        ["status", true, "locked"],
      ]);
    } finally {
      cleanup(relay, helper);
    }
  });

  test("WS 上行 ready 只下发一次 kind2；ping 立即 pong；need-key/set-bitrate 下行 helper；输入透传", async () => {
    const helperFrames: Buffer[] = [];
    // 收集告警：writeHelperControl 在 helper 未接通时按契约静默跳过（只 warn）——
    // 本用例把这类事件变响亮，避免"控制帧凭空消失"不可见（Linux CI 曾因此显现为时序 flake）。
    const warnings: string[] = [];
    const inputs: unknown[] = [];
    // 服务端接通信号：客户端 connect 不保证中继已登记 helper（kind3/4 会被 fail-soft 丢弃）——
    // 用中继的 onHelperConnected 钩子消除这一竞态（历史 Linux CI 时序 flake 的根因）。
    let resolveHelperConnected!: () => void;
    const helperConnected = new Promise<void>((resolve) => {
      resolveHelperConnected = resolve;
    });
    const relay = createRemoteDesktopMediaRelay({
      logger: {
        info(): void {},
        warn: (...args: unknown[]) => {
          warnings.push(args.map((arg) => String(arg)).join(" "));
        },
      },
      onFrame: () => {},
      onStatus: () => {},
      onReady: () => {},
      onInput: (e) => inputs.push(e),
      onHelperConnected: () => resolveHelperConnected(),
    });
    const helper = connect({ host: "127.0.0.1", port: await relay.listen() });
    try {
      await once(helper, "connect");
      await helperConnected;
      helper.on("data", (d) => {
        helperFrames.push(d);
      });
      const { ws: fakeWs, texts } = makeFakeWs();
      relay.attachWebSocket(fakeWs);
      relay.handleWsMessage(JSON.stringify({ t: "ready" }), false);
      // 重复 ready 必须幂等：一条会话只发一次开流指令
      relay.handleWsMessage(JSON.stringify({ t: "ready" }), false);
      relay.handleWsMessage(JSON.stringify({ t: "ping", ts: 123 }), false);
      relay.handleWsMessage(JSON.stringify({ t: "pm", x: 10, y: 20 }), false);
      // 控制面下行：need-key → kind3；set-bitrate → kind4 + u32BE 码率；非法码率丢弃
      relay.handleWsMessage(JSON.stringify({ t: "need-key" }), false);
      relay.handleWsMessage(JSON.stringify({ t: "set-bitrate", bps: 4_000_000 }), false);
      relay.handleWsMessage(JSON.stringify({ t: "set-bitrate", bps: 123 }), false);
      const expected = Buffer.concat([
        Buffer.from([0, 0, 0, 1, 2]), // kind2 开流
        Buffer.from([0, 0, 0, 1, 3]), // kind3 紧急关键帧
        Buffer.from([0, 0, 0, 5, 4, 0x00, 0x3d, 0x09, 0x00]), // kind4 + 4_000_000
      ]);
      await waitFor(
        () => Buffer.concat(helperFrames).length >= expected.length,
        "kind2+kind3+kind4 送达",
      );
      assert.deepEqual(
        Buffer.concat(helperFrames),
        expected,
        "ready 只下发一次 kind2；need-key/set-bitrate 按契约下行；非法码率不下行",
      );
      assert.equal(texts.length, 1, "仅一条 pong");
      const pong = JSON.parse(texts[0]!) as { t?: string; ts?: number; hostTs?: number };
      assert.equal(pong.t, "pong");
      assert.equal(pong.ts, 123, "原样回传 ts（RTT 基准）");
      assert.equal(typeof pong.hostTs, "number", "附带被控侧时钟戳（帧龄偏移标定用）");
      assert.deepEqual(inputs, [{ t: "pm", x: 10, y: 20 }], "控制面无匹配项之外的输入对象才透传 onInput");
      assert.ok(
        !warnings.some((warning) => warning.includes("helper not connected")),
        `不应有 helper 未接通告警：${warnings.join(" | ")}`,
      );
    } finally {
      cleanup(relay, helper);
    }
  });

  test("kind0 缺省出口：无 WS 丢弃；bufferedAmount>128KB 背压丢弃并请关键帧；低于阈值即发送", async () => {
    const helperFrames: Buffer[] = [];
    // defaultFrameSink 的发送异常被捕获后走 logger.warn——本用例收集告警并断言无发送失败
    // （noopLogger 会把这类异常吞掉，CI 上曾表现为「帧凭空消失」而不可见）。
    const warnings: string[] = [];
    // 服务端接通信号（同上用例）：等它再发丢帧，避免 kind3 请求撞进未接通窗口。
    let resolveHelperConnected!: () => void;
    const helperConnected = new Promise<void>((resolve) => {
      resolveHelperConnected = resolve;
    });
    const relay = createRemoteDesktopMediaRelay({
      logger: {
        info(): void {},
        warn: (...args: unknown[]) => {
          warnings.push(args.map((arg) => String(arg)).join(" "));
        },
      },
      // 不注入 onFrame → 走 relay 缺省出口（契约 C：断开即丢帧、>128KB 丢 kind0 + 请关键帧）
      onStatus: () => {},
      onReady: () => {},
      onInput: () => {},
      onHelperConnected: () => resolveHelperConnected(),
    });
    const helper = connect({ host: "127.0.0.1", port: await relay.listen() });
    try {
      await once(helper, "connect");
      await helperConnected;
      helper.on("data", (d) => {
        helperFrames.push(d);
      });
      const payloadA = Buffer.from([0, 0, 0, 1, 0, 0xaa]);
      const payloadB = Buffer.from([0, 0, 0, 1, 0, 0xbb]);
      const kind3 = Buffer.from([0, 0, 0, 1, 3]);

      // 未挂 WS：kind0 直接丢弃，不抛错（无从断言，只验证路径安全）
      helper.write(pipeFrame(0, payloadA));
      await sleep(20);

      const { ws: fakeWs, binaries, buffered } = makeFakeWs();
      buffered.amount = 200 * 1024;
      relay.attachWebSocket(fakeWs);
      helper.write(pipeFrame(0, payloadA));
      // 轮询替代固定 sleep：管道投递在 CI 负载下可能 >20ms（Linux verify 作业时序 flake 的修复）。
      await waitFor(() => Buffer.concat(helperFrames).length >= 5, "丢帧爆发触发 kind3");
      assert.equal(binaries.length, 0, "过 128KB 背压阈值应丢弃 kind0");
      assert.deepEqual(
        Buffer.concat(helperFrames),
        kind3,
        "丢帧爆发开始即向 helper 请求一次关键帧（kind3）",
      );

      // 同一爆发内继续丢弃：不重复请求（爆发标记去抖）——负向断言，短 settle 后仍只有一条。
      helper.write(pipeFrame(0, payloadA));
      await sleep(50);
      assert.deepEqual(
        Buffer.concat(helperFrames),
        kind3,
        "同一爆发只请求一次关键帧",
      );

      buffered.amount = 0;
      helper.write(pipeFrame(0, payloadB));
      // 轮询等待 payloadB 送达（若第二条 A 也迟到达则合法地先于 B 送出，故检查"含 B 且 B 完整"）。
      await waitFor(() => binaries.some((frame) => frame.equals(payloadB)), "低于阈值恢复发送");
      assert.deepEqual(binaries[binaries.length - 1], payloadB, "kind0 payload 原样转发（不解密不改写）");
      assert.ok(
        !warnings.some((warning) => warning.includes("send failed")),
        `不应有发送失败告警：${warnings.join(" | ")}`,
      );
    } finally {
      cleanup(relay, helper);
    }
  });

  test("超长帧防御：len > 16MB 关闭 helper 连接", async () => {
    const relay = createRemoteDesktopMediaRelay({
      logger: noopLogger,
      onStatus: () => {},
      onReady: () => {},
      onInput: () => {},
    });
    const helper = connect({ host: "127.0.0.1", port: await relay.listen() });
    try {
      await once(helper, "connect");
      const closed = once(helper, "close");
      const header = Buffer.alloc(4);
      header.writeUInt32BE(16 * 1024 * 1024 + 1, 0);
      helper.write(header);
      await Promise.race([closed, sleep(1000)]);
      assert.equal(helper.destroyed, true, "超长帧应导致连接被关闭");
    } finally {
      cleanup(relay, helper);
    }
  });

  test("ready 先于 helper 接入：开流指令挂起，helper 连通后补发且仍只发一次", async () => {
    const relay = createRemoteDesktopMediaRelay({
      logger: noopLogger,
      onFrame: () => {},
      onStatus: () => {},
      onReady: () => {},
      onInput: () => {},
    });
    const { ws: fakeWs } = makeFakeWs();
    const port = await relay.listen();
    let helper: Socket | undefined;
    try {
      // ready 先到（主控解码就绪可能早于 helper 管道接通，两者并行）
      relay.attachWebSocket(fakeWs);
      relay.handleWsMessage(JSON.stringify({ t: "ready" }), false);

      const helperFrames: Buffer[] = [];
      helper = connect({ host: "127.0.0.1", port });
      await once(helper, "connect");
      helper.on("data", (d) => {
        helperFrames.push(d);
      });
      await sleep(30);
      assert.deepEqual(
        Buffer.concat(helperFrames),
        Buffer.from([0, 0, 0, 1, 2]),
        "helper 接通后应补发完整 5 字节开流指令",
      );

      // 幂等不因补发而破坏：再收 ready 也不重发
      relay.handleWsMessage(JSON.stringify({ t: "ready" }), false);
      await sleep(30);
      assert.deepEqual(
        Buffer.concat(helperFrames),
        Buffer.from([0, 0, 0, 1, 2]),
        "补发后重复 ready 仍不重发",
      );
    } finally {
      cleanup(relay, helper);
    }
  });

  test("顶替旧 WS 收到 close(1000,'replaced')；同 socket 重复挂接不叠加监听；管道 len=0 断开", async () => {
    const relay = createRemoteDesktopMediaRelay({
      logger: noopLogger,
      onStatus: () => {},
      onReady: () => {},
      onInput: () => {},
    });
    const messageRegs: string[] = [];
    const closes: Array<{ code?: number; reason?: string }> = [];
    const ws1: WsLike = {
      send: () => {},
      on: (event) => {
        if (event === "message") {
          messageRegs.push(event);
        }
      },
      close: (code, reason) => {
        closes.push({ code, reason });
      },
    };
    const ws2: WsLike = { send: () => {}, on: () => {} };
    relay.attachWebSocket(ws1);
    assert.equal(messageRegs.length, 1, "首次挂接登记一个 message 监听");
    relay.attachWebSocket(ws1); // 同 socket 重复挂接：应短路，不叠监听
    assert.equal(messageRegs.length, 1, "同一 socket 重复挂接不叠加 message 监听");
    relay.attachWebSocket(ws2);
    assert.deepEqual(
      closes,
      [{ code: 1000, reason: "replaced" }],
      "被顶替的旧 WS 收到 1000/replaced",
    );

    // len=0（长度字段下界违规）同样断开 helper 连接；管道层无应用码，断言关闭事件即可
    const helper = connect({ host: "127.0.0.1", port: await relay.listen() });
    try {
      await once(helper, "connect");
      const closed = once(helper, "close");
      helper.write(Buffer.from([0, 0, 0, 0]));
      await Promise.race([closed, sleep(1000)]);
      assert.equal(helper.destroyed, true, "len=0 应导致连接被关闭");
    } finally {
      cleanup(relay, helper);
    }
  });

  test("方向越权/未知 kind 不外发不回调；close 幂等", async () => {
    const seen: unknown[] = [];
    const relay = createRemoteDesktopMediaRelay({
      logger: noopLogger,
      onFrame: (p) => seen.push(["frame", Buffer.from(p)]),
      onStatus: (frozen) => seen.push(["status", frozen]),
      onReady: () => seen.push(["ready"]),
      onInput: (e) => seen.push(["input", e]),
    });
    try {
      // kind2 只允许 host→helper；helper 侧收到只 warn 不转发
      relay.handlePipeData(pipeFrame(2, Buffer.alloc(0)));
      // 未知 kind：丢弃，不触发任何出口回调
      relay.handlePipeData(pipeFrame(9, Buffer.from([1, 2, 3])));
      // kind1 非法 JSON：丢弃，不触发 onStatus
      relay.handlePipeData(pipeFrame(1, Buffer.from("not-json")));
      assert.deepEqual(seen, []);

      await relay.listen();
      relay.close();
      relay.close(); // 幂等：重复关闭不抛
    } finally {
      cleanup(relay);
    }
  });

  test("剪贴板（P1.1）：kind5 采纳即下行 clip；重复值不重发；clip-mode 关闭后停止采纳", () => {
    const { ws: fakeWs, texts } = makeFakeWs();
    const relay = createRemoteDesktopMediaRelay({ logger: noopLogger });
    const clips = () => texts.map((text) => JSON.parse(text) as { t: string; text?: string });
    try {
      relay.attachWebSocket(fakeWs);
      relay.handlePipeData(pipeFrame(5, Buffer.from("来自被控", "utf8")));
      assert.deepEqual(clips(), [{ t: "clip", text: "来自被控" }], "首次变化即下行");
      // 同值重复上报（helper 轮询会反复读到同一文本）：门已登记，不重复下行。
      relay.handlePipeData(pipeFrame(5, Buffer.from("来自被控", "utf8")));
      assert.equal(texts.length, 1, "重复值不重发");
      // 开关关闭：新值不再采纳（关闭只停同步，不影响画面/输入）。
      relay.handleWsMessage(JSON.stringify({ t: "clip-mode", enabled: false }), false);
      relay.handlePipeData(pipeFrame(5, Buffer.from("关闭期的新值", "utf8")));
      assert.equal(texts.length, 1, "关闭后不采纳");
      // 重新打开：下一次变化照常下行（关闭期间的旧值不会补发）。
      relay.handleWsMessage(JSON.stringify({ t: "clip-mode", enabled: true }), false);
      relay.handlePipeData(pipeFrame(5, Buffer.from("重开后的新值", "utf8")));
      assert.deepEqual(clips().at(-1), { t: "clip", text: "重开后的新值" });
      // 非法开关载荷与空文本：丢弃且不改变后续行为。
      relay.handleWsMessage(JSON.stringify({ t: "clip-mode" }), false);
      relay.handlePipeData(pipeFrame(5, Buffer.alloc(0)));
      assert.equal(texts.length, 2, "非法载荷不产生下行");
    } finally {
      relay.close();
    }
  });

  test("server 收口回归：bye 关中继/端口释放/下一会话无双连接拒绝（tsx 子进程）", async () => {
    const child = spawn(process.execPath, ["--import", "tsx", fileURLToPath(import.meta.url)], {
      cwd: fileURLToPath(new URL("../../../", import.meta.url)),
      env: { ...process.env, [SERVER_CASE_ENV]: "1" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";
    child.stdout.on("data", (chunk) => (output += String(chunk)));
    child.stderr.on("data", (chunk) => (output += String(chunk)));
    const exitCode = await new Promise<number>((resolve, reject) => {
      const timer = setTimeout(() => {
        child.kill();
        reject(new Error(`server 收口场景 30s 未退出\n${output}`));
      }, 30_000);
      child.once("error", (error) => {
        clearTimeout(timer);
        reject(error);
      });
      child.once("close", (code) => {
        clearTimeout(timer);
        resolve(code ?? 1);
      });
    });
    assert.equal(exitCode, 0, `server 收口场景失败：\n${output}`);
    assert.match(output, /RD_MEDIA_RELAY_SERVER_CASE_OK/u, `场景未完成断言：\n${output}`);
  });
}
