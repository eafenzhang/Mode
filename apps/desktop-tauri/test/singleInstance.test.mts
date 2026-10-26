/**
 * M3 Task 4 · Step 1/2：**深链的单实例转发**（DR-T4-1）的断言。
 *
 * ## 这条用例要证的现状与目标
 *
 * 现状（M1 §5 登记、本任务修）：Windows 上应用已在运行时点 `mode://` 链接，操作系统会**起一个新进程**
 * ——`argv` 里带那条链接。今天那个新进程会走完整的启动路径：**第二个 Host、第二个窗口**，也就是
 * 第二份 workspace 状态。这与 `AGENTS.md` 的「每个窗口一个 window-scoped Local Host」相冲。
 *
 * 目标：第二个进程只做两件事——把 `argv` 里的深链交给**已在运行的实例**，然后**不建窗口、不建 Host**
 * 直接退出（退出码 0）。转发通道复用**既有**的深链事件通道（`mode:deep-link`），不另造 IPC。
 *
 * ## 判据是三条各自独立的观察
 *
 * 1. **第二进程没有起 Host**：它的 stdout/stderr 里不得出现 `host startupId=`（产品壳与 harness
 *    都在 `setup` 里、建窗口之前打这一行）；并且它必须**很快退出**且退出码为 0。
 * 2. **同时只有一个 Host**：按命令行匹配 `host-standalone` 的 `node.exe` 进程数必须**正好 1**
 *    （第二实例若起了自己的 Host，这个数是 2）。这条与第 1 条互为独立证据：一条看 B 自己说了什么，
 *    一条看进程表里实际有几个。
 * 3. **链接到达了已运行实例的渲染层**：A 的页面探针在 `window.mode.onOAuthCallback` 上收到
 *    `mode://oauth/callback?state=…` 并把它报给测试的 sink。
 *    这一条之所以在渲染层观察：转发链的终点是渲染层的订阅点
 *    （`argv → 壳的回调 → deeplink 路由 → 宿主 emit → window.mode 的订阅`），只看壳的日志
 *    证不了「链接到了页面里」。页面是 renderer 的构建产物（不许改），所以探针从注入侧订阅。
 *
 * ## 为什么 A 不是真产品二进制
 *
 * 第二个进程（B）用**真产品二进制**是更贴近现场的做法，但有两个硬问题：① 真壳的 `setup` 会
 * `deep_link().register()`，把 `HKCU\Software\Classes\mode` 的默认 handler 改指到调试产物
 * （M1 §4.3 记录过这个副作用，当时靠 `reg export`/`reg import` 还原）——在**修复前**（RED）
 * B 会走完整启动路径，于是这条用例会顺手改用户的注册表；② 真壳的窗口里没有探针（页面不许改），
 * 第 3 条判据就无从观察。所以 A 与 B 都取 `ProductUiHarness`——它用**同一批产品部件**
 * （`mode_tauri::single_instance` + `mode_tauri::host::start` + `mode_tauri::build_host_window`），
 * 只是多了一段探针注入。第二实例的**早退**发生在 `setup` 之前（插件 setup 钩子先于 app setup，
 * 见 spec 的窗口模型一节），所以 B 走的是哪条分支与它自己是不是产品壳无关。
 *
 * ## 为什么默认 skip
 *
 * 要真 webview + 真 Host + 真进程表。CI 跑不了（与 `webviewRoundTrip`/`productUiMount` 同性质），
 * 属**发版前人工跑一次**的那类。用例**默认**会跑第一条（纯静态的接线守卫，进 CI），
 * opt-in 只控制第二条。
 *
 * ```bash
 * MODE_TAURI_SINGLE_INSTANCE=1 node --import tsx --test apps/desktop-tauri/test/singleInstance.test.mts
 * ```
 */
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createServer, type Server } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const appDir = resolve(repoRoot, "apps/desktop-tauri");
const harnessBin = resolve(repoRoot, "target/debug/ProductUiHarness.exe");
/**
 * sink 端口：探针脚本里的上报地址是**固定的** `http://127.0.0.1:5177/product-ui-log`
 * （注入脚本不能从测试进程拿配置），所以这里必须与它一致。三个 opt-in 文件共用这个端口
 * ——它们本来就是一个个单独跑的（都要求真 webview），同一次 `--test` 里同时跑会互相抢端口。
 */
const SINK_PORT = 5177;
/** A 的看门狗（`MODE_PRODUCT_UI_TIMEOUT_MS`）：到点它自己退，`RunEvent::Exit` 收掉 Host。 */
const INSTANCE_TIMEOUT_MS = 45_000;
/** 等 A 的页面报告：产品 UI 要挂载（含 Host 启动），本机实测 8–15s。 */
const REPORT_TIMEOUT_MS = 120_000;
/** 第二实例必须在这段时间内退出；正常路径是毫秒级（转发后 `process::exit(0)`）。 */
const SECOND_INSTANCE_EXIT_MS = 15_000;
const DEEP_LINK_URL = "mode://oauth/callback?state=single-instance-forward";

const enabled = process.env.MODE_TAURI_SINGLE_INSTANCE === "1";

const read = (relativePath: string): string =>
  readFileSync(resolve(repoRoot, relativePath), "utf8");

// ---------------------------------------------------------------------------
// 静态守卫：接线结构（进 CI）
// ---------------------------------------------------------------------------

const MAIN_RS = "apps/desktop-tauri/src-tauri/src/main.rs";
const SINGLE_INSTANCE_RS = "apps/desktop-tauri/src-tauri/src/single_instance.rs";
const DEEPLINK_LIB_RS = "crates/deeplink/src/lib.rs";

test("单实例接线：插件最先注册、第二实例的转发走既有深链事件通道、且不建窗口", () => {
  const main = read(MAIN_RS);
  const registration = main.indexOf("single_instance::install(");
  assert.ok(
    registration >= 0,
    `${MAIN_RS} 必须通过 mode_tauri::single_instance::install(...) 注册单实例插件——` +
      `它是「第二实例不建窗口、不建 Host」的唯一来源`,
  );

  // ① 插件在 builder 链上必须**先于** app setup：第二实例的早退发生在插件 setup 钩子里，
  //    而窗口与 Host 都在 app setup 里建。文本上的对应关系就是「注册在 .setup( 之前」。
  const appSetup = main.indexOf(".setup(");
  assert.ok(appSetup >= 0, `${MAIN_RS} 里应有 app setup`);
  assert.ok(
    registration < appSetup,
    `${MAIN_RS} 里单实例插件必须注册在 \`.setup(\` **之前**（插件的 setup 钩子先执行，` +
      `第二实例在钩子里就退出了；注册晚了就来不及阻止它建窗口与 Host）`,
  );

  // ② 建窗口与起 Host 仍然**只在** app setup 里 —— 这才是「早退 = 什么都不建」成立的前提。
  for (const call of ["mode_tauri::host::start(", "mode_tauri::build_host_window("]) {
    const at = main.indexOf(call);
    assert.ok(at >= 0, `${MAIN_RS} 里应有 ${call}`);
    assert.ok(at > appSetup, `${MAIN_RS} 里 ${call} 必须在 app setup 之内`);
  }

  // ③ 转发通道是**既有的**深链事件通道，不是新 IPC：壳的单实例模块必须把 argv 交给
  //    deeplink crate 的转发入口（它内部 dispatch 到同一个 router，再 emit 同一个事件名）。
  const singleInstance = read(SINGLE_INSTANCE_RS);
  assert.ok(
    singleInstance.includes("deeplink::forward_second_instance_args"),
    `${SINGLE_INSTANCE_RS} 必须把 argv 交给 deeplink::forward_second_instance_args` +
      `（复用既有的 DeepLinkRouter + mode:deep-link 事件；另造 IPC 会让渲染层多一条通道）`,
  );
  // ④ 转发路径不得自己建窗口——单实例的全部意义就是「第二实例不建第二个窗口」。
  for (const forbidden of ["build_host_window", "WebviewWindowBuilder", "host::start"]) {
    assert.ok(
      !singleInstance.includes(forbidden),
      `${SINGLE_INSTANCE_RS} 里不得出现 ${forbidden}（转发路径只允许聚焦既有窗口）`,
    );
  }
  // ⑤ deeplink 侧的转发入口与 on_open_url 走**同一个**投递实现（不是第二套 emit）。
  const deeplinkLib = read(DEEPLINK_LIB_RS);
  assert.ok(
    deeplinkLib.includes("pub fn forward_second_instance_args"),
    `${DEEPLINK_LIB_RS} 里应有 pub fn forward_second_instance_args`,
  );
  assert.ok(
    deeplinkLib.includes("fn dispatch_raw"),
    `${DEEPLINK_LIB_RS} 里应有共享的 fn dispatch_raw（on_open_url 与第二实例转发共用一条投递路径）`,
  );
});

// ---------------------------------------------------------------------------
// opt-in 的端到端
// ---------------------------------------------------------------------------

interface ProbeReport {
  status: string;
  expect?: string;
  mounted?: boolean;
  phase?: string | null;
  startupId?: string | null;
  deepLinkSubscribed?: boolean | null;
  deepLinkUrls?: string[];
  elapsedMs?: number;
}

interface Sink {
  server: Server;
  reports: ProbeReport[];
  lines: string[];
}

function startSink(): Promise<Sink> {
  return new Promise((resolveSink, rejectSink) => {
    const sink: Sink = { server: undefined as unknown as Server, reports: [], lines: [] };
    const server = createServer((request, response) => {
      const url = new URL(request.url ?? "/", `http://127.0.0.1:${SINK_PORT}`);
      response.setHeader("Access-Control-Allow-Origin", "*");
      response.setHeader("Access-Control-Allow-Methods", "GET, OPTIONS");
      const line = url.searchParams.get("line");
      const report = url.searchParams.get("report");
      if (line) {
        sink.lines.push(line);
        console.log(`SINGLE_INSTANCE_PAGE_LOG ${line}`);
      }
      if (report) {
        const parsed = JSON.parse(report) as ProbeReport;
        sink.reports.push(parsed);
        console.log(`SINGLE_INSTANCE_REPORT ${report}`);
      }
      response.statusCode = 204;
      response.end();
    });
    server.on("error", rejectSink);
    server.listen(SINK_PORT, "127.0.0.1", () => {
      sink.server = server;
      resolveSink(sink);
    });
  });
}

async function closeSink(sink: Sink): Promise<void> {
  await new Promise<void>((resolveClose) => {
    sink.server.close(() => resolveClose());
    setTimeout(resolveClose, 2_000);
  });
}

function run(command: string, args: string[], cwd: string): void {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" });
  assert.equal(result.status, 0, `命令失败：${command} ${args.join(" ")}`);
}

/** 装配前端根（真 renderer 产物）——它会先清 `dist/`，所以必须在构建 harness 之前跑。 */
function assembleFrontend(): void {
  run(
    process.execPath,
    ["--import", "tsx", resolve(appDir, "scripts/build-renderer-for-tauri.ts"), "--no-bench"],
    repoRoot,
  );
  assert.ok(
    existsSync(join(appDir, "dist/index.html")),
    "装配后的 dist 里必须有产品页 index.html",
  );
}

function buildHarnessBin(): void {
  run(
    process.execPath,
    [
      resolve(repoRoot, "scripts/rust-cargo.ts"),
      "build",
      "-p",
      "mode-tauri",
      "--features",
      "product-ui-harness",
      "--bin",
      "ProductUiHarness",
    ],
    repoRoot,
  );
  assert.ok(existsSync(harnessBin), `找不到产物 ${harnessBin}`);
}

const delay = (ms: number): Promise<void> =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

async function waitForExit(child: ChildProcess, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (child.exitCode === null && Date.now() < deadline) {
    await delay(200);
  }
}

/**
 * 数**本机**有几个 mode-tauri 二进制在跑。
 *
 * 为什么要先查：单实例插件的互斥体是按 app identifier（`dev.mode.app`）建的，产品壳与三个
 * opt-in harness **共用同一个 identifier**。另一个实例在场时，B 的转发会交给**它**，
 * 症状是本用例报「A 没收到链接」——而原因在环境里，不在代码里。
 */
function runningModeTauriInstances(): string[] {
  const images = [
    "ModeTauri.exe",
    "ProductUiHarness.exe",
    "HostRoundtripHarness.exe",
    "PortTransportHarness.exe",
  ];
  const running: string[] = [];
  for (const image of images) {
    const result = spawnSync("tasklist", ["/FI", `IMAGENAME eq ${image}`, "/NH", "/FO", "CSV"], {
      encoding: "utf8",
    });
    const lines = (result.stdout ?? "").split("\n");
    if (lines.some((line) => line.toLowerCase().startsWith(`"${image.toLowerCase()}"`))) {
      running.push(image);
    }
  }
  return running;
}

/** 命令行里带 `host-standalone` 的 node 进程数（= 本机的 Host sidecar 数）。 */
function hostProcessCount(): number {
  const script =
    "@(Get-CimInstance Win32_Process -Filter \"Name='node.exe'\" | " +
    "Where-Object { $_.CommandLine -like '*host-standalone*' }).Count";
  const result = spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], {
    encoding: "utf8",
  });
  assert.equal(result.status, 0, `查询 Host 进程数失败：${result.stderr ?? ""}`);
  const count = Number((result.stdout ?? "").trim());
  assert.ok(Number.isInteger(count), `Host 进程数不是整数：${JSON.stringify(result.stdout)}`);
  return count;
}

interface Instance {
  child: ChildProcess;
  output: string;
  sink: Sink;
  sandbox: string;
  dataBaseDir: string;
}

/** 起一个实例（A 或 B）：sink 由调用方传，`argv` 就是进程参数。 */
function spawnInstance(
  sink: Sink,
  sandbox: string,
  dataBaseDir: string,
  argv: string[],
): Instance {
  const home = join(sandbox, "home");
  const instance: Instance = {
    child: undefined as unknown as ChildProcess,
    output: "",
    sink,
    sandbox,
    dataBaseDir,
  };
  const child = spawn(harnessBin, argv, {
    cwd: appDir,
    env: {
      ...process.env,
      MODE_HOST_SIDECAR_ENTRY: resolve(repoRoot, "packages/desktop/src/host-standalone/index.ts"),
      MODE_DATA_BASE_DIR: dataBaseDir,
      HOME: home,
      USERPROFILE: home,
      MODE_PRODUCT_UI_TIMEOUT_MS: String(INSTANCE_TIMEOUT_MS),
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout?.on("data", (chunk: Buffer) => {
    instance.output += chunk.toString("utf8");
    process.stdout.write(chunk);
  });
  child.stderr?.on("data", (chunk: Buffer) => {
    instance.output += chunk.toString("utf8");
    process.stderr.write(chunk);
  });
  instance.child = child;
  return instance;
}

/** 尽力删沙箱：刚退出的进程可能还握着句柄（Windows 实测过 `EPERM`）。失败不影响断言。 */
function removeSandboxBestEffort(sandbox: string): void {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      rmSync(sandbox, { recursive: true, force: true });
      return;
    } catch {
      spawnSync(process.execPath, ["-e", "setTimeout(function(){}, 300)"], { stdio: "ignore" });
    }
  }
  console.warn(`[single-instance] 沙箱未能删除（不影响断言）：${sandbox}`);
}

function assertNoProcess(pid: number, what: string): void {
  const tasklist = spawnSync("tasklist", ["/FI", `PID eq ${pid}`, "/NH", "/FO", "CSV"], {
    encoding: "utf8",
  });
  assert.ok(!tasklist.stdout.includes(`"${pid}"`), `${what}（pid ${pid}）`);
}

test(
  "第二实例把深链交给已运行实例后退出：不起第二个 Host、不建窗口，链接到达渲染层",
  {
    skip: enabled
      ? false
      : "opt-in：设 MODE_TAURI_SINGLE_INSTANCE=1 才跑（要真 webview + 真进程表）",
  },
  async () => {
    const others = runningModeTauriInstances();
    assert.deepEqual(
      others,
      [],
      `本机已有 mode-tauri 实例在跑：${others.join(", ")}——互斥体按 app identifier 共用，` +
        `它们在场的转发会交给它们，测出来的不是这条用例的对象。先关掉再跑`,
    );

    assembleFrontend();
    buildHarnessBin();

    const sink = await startSink();
    const sandbox = mkdtempSync(join(tmpdir(), "mode-single-instance-"));
    const dataBaseDir = join(sandbox, "data");
    let a: Instance | undefined;
    let b: Instance | undefined;
    try {
      // ---- A：已在运行的实例（产品部件 + 探针注入）----
      a = spawnInstance(sink, sandbox, dataBaseDir, []);
      const reportDeadline = Date.now() + REPORT_TIMEOUT_MS;
      while (sink.reports.length === 0 && Date.now() < reportDeadline) {
        await delay(250);
      }
      assert.ok(
        sink.reports.length > 0,
        `A 的页面没有报告（它必须真的挂载，否则「链接到达渲染层」无从观察）：` +
          `日志：\n${sink.lines.join("\n")}\n输出：\n${a.output}`,
      );
      const report = sink.reports[0]!;
      assert.equal(report.mounted, true, "A 必须真的挂上产品 UI");
      // 订阅没装上的话，「转发没到」与「探针没订上」在下面的断言里长得一模一样——先钉住后者。
      assert.equal(
        report.deepLinkSubscribed,
        true,
        `A 的探针必须装上深链订阅（否则第 3 条判据是空的）：${JSON.stringify(report)}`,
      );
      assert.equal(
        hostProcessCount(),
        1,
        "A 起来之后应当只有**一个** Host（判定基线）",
      );

      // ---- B：第二实例（操作系统的行为：同一个 exe + 深链在 argv 里）----
      b = spawnInstance(sink, sandbox, dataBaseDir, [DEEP_LINK_URL]);
      await waitForExit(b.child, SECOND_INSTANCE_EXIT_MS);
      const bExited = b.child.exitCode !== null;
      const bOutput = b.output;

      // ① 第二进程不得起 Host（产品壳与 harness 都在建窗口之前打 `host startupId=`）。
      assert.ok(
        !bOutput.includes("host startupId="),
        `第二实例**不得**起第二个 Host，实际输出：\n${bOutput}`,
      );
      // ② 不得走完 setup（窗口就是在 setup 里建的）。
      assert.ok(
        !bOutput.includes("HARNESS injection="),
        `第二实例不得建窗口（setup 里的 injection 那行不该出现），实际输出：\n${bOutput}`,
      );
      // ③ 必须自己退出，且退出码为 0（转发完成即 `process::exit(0)`）。
      assert.equal(
        bExited,
        true,
        `${SECOND_INSTANCE_EXIT_MS}ms 内第二实例必须退出（转发完就走），实际输出：\n${bOutput}`,
      );
      assert.equal(b.child.exitCode, 0, `第二实例的退出码应为 0，实际 ${String(b.child.exitCode)}`);

      // ④ 进程表里仍然**只有一个** Host（与 ① 互为独立证据）。
      assert.equal(
        hostProcessCount(),
        1,
        "第二实例起来之后 Host 数必须仍是 1（起了自己的 Host 就会变成 2）",
      );

      // ⑤ 链接到达了**已运行实例的渲染层**：A 的探针在 window.mode 的订阅点上收到了它。
      assert.ok(
        sink.lines.some((line) => line.includes(`deep_link_received url=${DEEP_LINK_URL}`)),
        `转发进来的深链必须到达 A 的渲染层（探针订阅 window.mode.onOAuthCallback 观察）：\n` +
          `日志：\n${sink.lines.join("\n")}\nA 输出：\n${a.output}\nB 输出：\n${bOutput}`,
      );
    } finally {
      for (const instance of [b, a]) {
        if (!instance) continue;
        // A 走自己的看门狗退出（`RunEvent::Exit` 收掉 Host）；B 正常情况下早就退过了。
        await waitForExit(instance.child, 60_000);
        if (instance.child.exitCode === null) instance.child.kill();
      }
      await delay(500);
      const hostPid = /HARNESS host startupId=\S+ pid=(\d+)/.exec(a?.output ?? "")?.[1];
      if (hostPid) assertNoProcess(Number(hostPid), "A 退出后不得残留 Host 进程");
      await closeSink(sink);
      removeSandboxBestEffort(sandbox);
    }
  },
);
