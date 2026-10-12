/**
 * host 侧输入换算链集成回归（Task 14 fix round 1）：契约 F 的 MediaStart/MediaSwitch
 * 携带 monitor rect + virtualRect → media WS 上行 renderer 事件（{t:...}，显示器内归一化）
 * → host 单跳换算 → raw_input 帧（{k:...}，VIRTUALDESK 绝对）→ 注入端口。
 *
 * 运行：node --test packages/desktop/tests/remote-desktop-input-path.test.mts
 * server 级装配只有 tsx 能解析 .js 后缀与 @mode 别名，故用例经 `node --import tsx` 子进程执行
 * （与 remote-desktop-media-relay.test.mts 的 server 收口场景同口径）。
 * 期望值口径：shared `mapMonitorNormalizedToVirtualAbsolute`（0..65535，含多屏负原点），
 * 关键对照 = 同一条指针在同一 monitor 下，virtual 是否被消费会改变结果（49156 vs 32768）。
 */
import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

/** 子进程模式开关：值为 "1" 时本文件直接跑场景，不注册外层测试。 */
const CASE_ENV = "RD_INPUT_PATH_CASE";

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/** 轮询等待条件成立（5s 上限）：注入端口是 fire-and-forget，无事件可 await。 */
async function waitFor(predicate: () => boolean, what: string): Promise<void> {
  const deadline = Date.now() + 5_000;
  while (!predicate()) {
    if (Date.now() > deadline) {
      assert.fail(`等待超时：${what}`);
    }
    await sleep(10);
  }
}

/** 可记录 on 注册与 close 调用的 fake socket（与 relay 测试同款最小形态）。 */
function makeFakeSocket(): {
  on(event: string, cb: (...args: never[]) => void): void;
  send(data: unknown): void;
  close(code?: number, reason?: string): void;
  fire(event: string, ...args: unknown[]): void;
} {
  const handlers = new Map<string, Array<(...args: unknown[]) => void>>();
  return {
    on(event, cb) {
      const list = handlers.get(event) ?? [];
      list.push(cb as (...args: unknown[]) => void);
      handlers.set(event, list);
    },
    send() {},
    close() {
      for (const cb of handlers.get("close") ?? []) {
        cb();
      }
    },
    fire(event, ...args) {
      for (const cb of handlers.get(event) ?? []) {
        cb(...args);
      }
    },
  };
}

async function runInputPathCase(): Promise<void> {
  // 剔除环境 kill-switch，保证准入只看被控开关（下面 mock 恒开）。
  delete process.env.MODE_REMOTE_DESKTOP_KILL_SWITCH;
  const { createRemoteDesktopHostServer } = await import("../src/host/remoteDesktopServer.js");
  const { RemoteDesktopHostMessageTypes } = await import("../src/host/remoteDesktopMessages.js");

  const injectedFrames: unknown[][] = [];
  const warns: string[] = [];
  const posted: Array<{ type?: string }> = [];
  const host = createRemoteDesktopHostServer({
    credentials: {
      load: async (key: string) => (key === "remote-desktop:server-enabled" ? "1" : "[]"),
      save: async () => {},
      delete: async () => {},
      list: async () => [],
    },
    logger: {
      info: () => {},
      warn: (...args: unknown[]) => warns.push(args.map(String).join(" ")),
    },
    postToMain: (message) => {
      posted.push(message);
    },
    // 注入端口测试替身：记录每帧（缺省实现会直连 broker socket，测试环境不可达）。
    injectRawInput: async (events) => {
      injectedFrames.push(events as unknown[]);
      return { dispatched: events.length };
    },
    getActiveLanAccessServer: () =>
      ({
        getState: () => ({
          clients: [{ id: "client-a", label: "Office-PC", remoteDesktopAuthorized: true }],
        }),
        setClientRemoteDesktopAuthorized: async () => {},
      }) as never,
  });
  await host.loadServerState();

  // 同意通过 → 信令会话 + 媒体 WS 挂接（单会话门：无 activeSession 时 inject 也不出）。
  const request = host.hooks.requestSession("client-a");
  await waitFor(
    () =>
      posted.some(
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
  const sessionSocket = makeFakeSocket();
  host.hooks.onSessionSocket("client-a", sessionSocket);
  const mediaSocket = makeFakeSocket();
  host.hooks.onMediaSocket("client-a", mediaSocket);

  const upstream = (payload: unknown): void => {
    mediaSocket.fire("message", Buffer.from(JSON.stringify(payload)));
  };

  // ① MediaStart 之前（无换算基准）：输入丢弃并计数，不进注入端口。
  upstream({ t: "pm", x: 0.5, y: 0.5 });
  await sleep(30);
  assert.equal(injectedFrames.length, 0, "MediaStart 前的输入不得注入");

  // ② 开流：主屏采集（物理 0..1920），虚拟桌面 = 主屏 + 左侧副屏（双屏 union，含负原点）。
  host.handleMessage({
    type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
    rect: { x: 0, y: 0, width: 1920, height: 1080 },
    virtualRect: { x: -1920, y: 0, width: 3840, height: 1080 },
    key: "a".repeat(64),
    salt: "b".repeat(16),
  });
  await sleep(30);

  // ③ pointer：主屏中心归一化 (0.5,0.5) → 虚拟桌面（3840 宽、原点 -1920）中轴右侧。
  // 期望 49156 证明 virtualRect 被消费（若误用 monitor 当 virtual 会得 32768）。
  upstream({ t: "pm", x: 0.5, y: 0.5 });
  await waitFor(() => injectedFrames.length >= 1, "pointer 事件注入");
  await sleep(30);
  assert.deepEqual(injectedFrames[0], [{ k: "move", x: 49156, y: 32768 }], "pointer 归一化坐标换算");

  // ④ control：按钮（带坐标）、滚轮（刻度归一）、键与文本（不带坐标）。
  upstream({ t: "md", button: "left", x: 1, y: 1 });
  await waitFor(() => injectedFrames.length >= 2, "按钮事件注入");
  assert.deepEqual(
    injectedFrames[1],
    [{ k: "button", button: "left", down: true, x: 65535, y: 65535 }],
    "按钮事件换算（主屏右边界 → 虚拟桌面右端点）",
  );
  upstream({ t: "mw", direction: "up", delta: 100, x: 0, y: 0 });
  await waitFor(() => injectedFrames.length >= 3, "滚轮事件注入");
  assert.deepEqual(
    injectedFrames[2],
    [{ k: "wheel", direction: "up", amount: 1, x: 32776, y: 0 }],
    "滚轮刻度归一 + 定位坐标",
  );
  upstream({ t: "kd", vk: 65 });
  await waitFor(() => injectedFrames.length >= 4, "按键事件注入");
  assert.deepEqual(injectedFrames[3], [{ k: "key", vk: 65, down: true }], "按键事件（VK 口径）");
  upstream({ t: "tx", text: "中文" });
  await waitFor(() => injectedFrames.length >= 5, "文本事件注入");
  assert.deepEqual(injectedFrames[4], [{ k: "text", text: "中文" }], "文本事件（Unicode 口径）");

  // ⑤ 畸形/未知形状与越界坐标：schema 拒收、不进端口（上行是不可信面）。
  const beforeInvalid = injectedFrames.length;
  upstream({ t: "nope" });
  upstream({ t: "pm", x: 2, y: 0 });
  upstream({ k: "move", x: 0, y: 0 });
  await sleep(30);
  assert.equal(injectedFrames.length, beforeInvalid, "畸形/越界/raw 形状的上行一律不注入");

  // ⑥ MediaSwitch 换屏：换算基准立刻换到左侧副屏（同一虚拟桌面），同一条 (1,1) 落到副屏右缘。
  host.handleMessage({
    type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaSwitch,
    rect: { x: -1920, y: 0, width: 1920, height: 1080 },
    virtualRect: { x: -1920, y: 0, width: 3840, height: 1080 },
  });
  await sleep(30);
  upstream({ t: "pm", x: 1, y: 1 });
  await waitFor(() => injectedFrames.length >= beforeInvalid + 1, "换屏后 pointer 注入");
  assert.deepEqual(
    injectedFrames[beforeInvalid],
    [{ k: "move", x: 32759, y: 65535 }],
    "换屏后按新 monitor rect 换算（32759 = 副屏右缘落在虚拟桌面中轴左侧）",
  );

  // ⑦ 丢帧 warn 纪律：只在首个丢弃时 warn（60fps 输入面不逐条刷屏）。
  assert.ok(
    warns.some((line) => line.includes("no coordinate reference yet")),
    "MediaStart 前的丢弃应有一次 warn",
  );
  assert.equal(
    warns.filter((line) => line.includes("media ws input dropped")).length,
    1,
    "多个丢弃合计只 warn 一次（计数不刷屏）",
  );

  host.dispose();
}

if (process.env[CASE_ENV] === "1") {
  await runInputPathCase();
  console.log("RD_INPUT_PATH_CASE_OK");
} else {
  test("host 输入换算链：MediaStart/Switch 的 rect+virtualRect → renderer 事件 → raw_input 帧（tsx 子进程）", async () => {
    const child = spawn(process.execPath, ["--import", "tsx", fileURLToPath(import.meta.url)], {
      cwd: fileURLToPath(new URL("../../../", import.meta.url)),
      env: { ...process.env, [CASE_ENV]: "1" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";
    child.stdout.on("data", (chunk) => (output += String(chunk)));
    child.stderr.on("data", (chunk) => (output += String(chunk)));
    const exitCode = await new Promise<number>((resolve, reject) => {
      const timer = setTimeout(() => {
        child.kill();
        reject(new Error(`输入换算场景 30s 未退出\n${output}`));
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
    assert.equal(exitCode, 0, `输入换算场景失败：\n${output}`);
    assert.match(output, /RD_INPUT_PATH_CASE_OK/u, `场景未完成断言：\n${output}`);
  });
}
