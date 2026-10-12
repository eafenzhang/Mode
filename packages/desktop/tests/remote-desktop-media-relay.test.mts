/* eslint-disable max-lines -- 测试文件堆叠分帧/背压/收口场景；oxlint 的 test 豁免（.oxlintrc.json）只匹配 *.test.ts 未覆盖 .mts，本豁免与其同一口径。 */
/**
 * host 媒体中继回归：本地管道长度前缀分帧、kind 路由、media WS 上行（ready/ping/输入）、
 * 背压丢弃与超长帧防御。运行：
 * node --test packages/desktop/tests/remote-desktop-media-relay.test.mts
 * 扩展名 .mts：语言政策新测试禁 .mjs（docs/specs/language-policy.md）。
 * 直接 import src 用 .ts 扩展名（与 remote-workspace-session-services.test.mts 同口径）：
 * node --test 原生 type stripping 不做 .js → .ts 重写，简报里的 .js 写法在该入口下无法解析。
 * 每个用例用 try/finally 收口：断言失败时也必须关掉回环连接，否则进程句柄挂住不退。
 *
 * server 级收口场景（bye 关中继回归）经 RD_MEDIA_RELAY_SERVER_CASE=1 的 tsx 子进程执行：
 * remoteDesktopServer 的 .js 后缀与 @mode 别名只有 tsx 能解析，node --test 原生剥离不行。
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
  /** T14 接线证据：注入端口的调用记录（media WS 上行 → relay onInput → inject → 本端口）。 */
  const injectedFrames: unknown[][] = [];
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
    // T14 注入端口测试替身：记录每帧（缺省实现会直连 broker socket，测试环境不可达）。
    injectRawInput: async (events) => {
      injectedFrames.push(events as unknown[]);
      return { dispatched: events.length };
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

  // ── T14 接线证据：media WS 上行输入 → relay onInput → inject → 注入端口 ──
  // raw_input 同形事件必须到达注入端口；非 raw_input 形状（契约 C 的 {t:...} 归一化事件）
  // 由形状门丢弃、不进端口（形状/坐标换算链落地前的口径，见 task-14 报告）。
  mediaSocket.fire("message", Buffer.from(JSON.stringify({ k: "move", x: 12345, y: 6789 })));
  mediaSocket.fire("message", Buffer.from(JSON.stringify({ t: "pm", x: 0.5, y: 0.5 })));
  await waitFor(() => injectedFrames.length > 0, "media WS 上行输入到达注入端口");
  await sleep(30);
  assert.deepEqual(
    injectedFrames,
    [[{ k: "move", x: 12345, y: 6789 }]],
    "只有一个 raw_input 同形事件被注入（{t:...} 事件被形状门丢弃）",
  );

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

  test("WS 上行 ready 只下发一次 kind2；ping 立即 pong；输入透传", async () => {
    const helperFrames: Buffer[] = [];
    const inputs: unknown[] = [];
    const relay = createRemoteDesktopMediaRelay({
      logger: noopLogger,
      onFrame: () => {},
      onStatus: () => {},
      onReady: () => {},
      onInput: (e) => inputs.push(e),
    });
    const helper = connect({ host: "127.0.0.1", port: await relay.listen() });
    try {
      await once(helper, "connect");
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
      await sleep(30);
      assert.deepEqual(
        Buffer.concat(helperFrames),
        Buffer.from([0, 0, 0, 1, 2]),
        "ready 只应下发一次完整 5 字节开流指令 [u32BE 1][0x02]",
      );
      assert.deepEqual(texts, ['{"t":"pong","ts":123}'], "ping 立即回 pong 且原样回传 ts");
      assert.deepEqual(inputs, [{ t: "pm", x: 10, y: 20 }], "非 ready/ping 的上行对象透传 onInput");
    } finally {
      cleanup(relay, helper);
    }
  });

  test("kind0 缺省出口：无 WS 丢弃；bufferedAmount>4MB 背压丢弃；低于阈值即发送", async () => {
    const relay = createRemoteDesktopMediaRelay({
      logger: noopLogger,
      // 不注入 onFrame → 走 relay 缺省出口（契约 C：断开即丢帧、>4MB 丢 kind0）
      onStatus: () => {},
      onReady: () => {},
      onInput: () => {},
    });
    const helper = connect({ host: "127.0.0.1", port: await relay.listen() });
    try {
      await once(helper, "connect");
      const payloadA = Buffer.from([0, 0, 0, 1, 0, 0xaa]);
      const payloadB = Buffer.from([0, 0, 0, 1, 0, 0xbb]);

      // 未挂 WS：kind0 直接丢弃，不抛错（无从断言，只验证路径安全）
      helper.write(pipeFrame(0, payloadA));
      await sleep(20);

      const { ws: fakeWs, binaries, buffered } = makeFakeWs();
      buffered.amount = 5 * 1024 * 1024;
      relay.attachWebSocket(fakeWs);
      helper.write(pipeFrame(0, payloadA));
      await sleep(20);
      assert.equal(binaries.length, 0, "过 4MB 背压阈值应丢弃 kind0");

      buffered.amount = 0;
      helper.write(pipeFrame(0, payloadB));
      await sleep(20);
      assert.equal(binaries.length, 1, "低于阈值应恢复发送");
      assert.deepEqual(binaries[0], payloadB, "kind0 payload 原样转发（不解密不改写）");
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
