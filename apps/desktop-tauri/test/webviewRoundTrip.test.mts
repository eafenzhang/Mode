/**
 * **opt-in** 集成测试：在真 webview 里验「产品壳 + 产品 Host」的两条端到端路径。
 *
 * 为什么默认 skip（DR-T1-2 的第二层）：这两条路径都要真 webview，CI 里跑不了——
 * `initialization_script` 的注入时机、`window.postMessage(payload, "*", [port])` 的转移与
 * `event.source === window`、ACL/本地内容、Tauri raw 响应的真实字节通路，四件事都只有真窗口能给。
 * Task 0 的 spike 已证明它们可行（`docs/plans/2026-10-16-m3-port-transport-spike.md` §3），
 * 本文件把它们落到**产品件**上：桥来自 `crates/sidecar-host/src/bridge.ts`，Host 来自
 * `packages/desktop/src/host-standalone/index.ts`，页面用未改动的
 * `DatabaseStartupAdmission` 当闸门。
 *
 * 两条路径（M3 Task 2 · Step 4）：
 *
 * 1. **成功**：真启动跑完 → 壳推 `ready` → 闸门放行 → 端口上完成一次真 RPC；
 * 2. **失败**：真启动失败（基目录被文件占住）→ Host 就绪前退出 → 壳的 reaper 推
 *    `failed(transport_closed)` → 页面必须**收到** `failed` + `errorCode`，且闸门**不放行**。
 *    第 2 条是「渲染层在收到第一条状态后没有自己的超时」这条口径的端到端兑现：失败只能由壳推。
 *
 * **这是发布流程上的硬要求**：发版前必须人工跑一次（漏跑 = 端口路径悄悄坏掉而流程说全绿）。
 * 命令与判据见 `docs/specs/tauri-renderer-host.md` 的「覆盖分工」。
 *
 * ```bash
 * MODE_TAURI_WEBVIEW_ROUNDTRIP=1 node --import tsx --test apps/desktop-tauri/test/webviewRoundTrip.test.mts
 * ```
 * 用例自己完成三件准备：构建产品前端 + 集成页（Rust 编译期内嵌 `frontendDist`，顺序不能倒）、
 * 构建带 `host-roundtrip` feature 的 harness bin、起一个 HTTP sink 收页面报告。
 */
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createServer, type Server } from "node:http";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const appDir = resolve(repoRoot, "apps/desktop-tauri");
const viteBin = resolve(repoRoot, "node_modules/vite/bin/vite.js");
const harnessBin = resolve(repoRoot, "target/debug/HostRoundtripHarness.exe");
const SINK_PORT = 5177;
const ROUNDTRIP_TIMEOUT_MS = 25_000;
const REPORT_TIMEOUT_MS = 90_000;

const enabled = process.env.MODE_TAURI_WEBVIEW_ROUNDTRIP === "1";

interface PageReport {
  status: string;
  gateOpened?: boolean;
  portIsRealMessagePort?: boolean;
  initializeObserved?: boolean;
  schemaAccepted?: boolean;
  statePhase?: string | null;
  stateStartupId?: string | null;
  startupIdMatches?: boolean;
  failedPhase?: string | null;
  errorCode?: string | null;
  portSeen?: boolean;
  describe?: {
    name?: string;
    pid?: number;
    dataBaseDir?: string;
    dataRoot?: string;
    startupId?: string;
    serviceChannels?: string[];
  };
  message?: string;
  bridge?: unknown;
}

interface Sink {
  server: Server;
  reports: PageReport[];
  lines: string[];
}

/** 起日志 sink（页面把报告 fetch 到这里，独立于 Tauri invoke）。 */
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
        console.log(`ROUNDTRIP_PAGE_LOG ${line}`);
      }
      if (report) {
        const parsed = JSON.parse(report) as PageReport;
        sink.reports.push(parsed);
        console.log(`ROUNDTRIP_PAGE_REPORT ${report}`);
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

/** 用仓库自带的 vite 构建一个配置（不经过 pnpm：路径与版本都确定）。 */
function buildFrontend(configPath: string): void {
  const result = spawnSync(process.execPath, [viteBin, "build", "--config", configPath], {
    cwd: appDir,
    stdio: "inherit",
  });
  assert.equal(result.status, 0, `vite build 失败：${configPath}`);
}

/** 用本机 cargo 包装器构建 opt-in harness bin（已是最新时 cargo 自己跳过重编）。 */
function buildHarnessBin(): void {
  const result = spawnSync(
    process.execPath,
    [
      resolve(repoRoot, "scripts/rust-cargo.ts"),
      "build",
      "-p",
      "mode-tauri",
      "--features",
      "host-roundtrip",
      "--bin",
      "HostRoundtripHarness",
    ],
    { cwd: repoRoot, stdio: "inherit" },
  );
  assert.equal(result.status, 0, "构建 HostRoundtripHarness 失败");
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

interface HarnessRun {
  report: PageReport;
  lines: string[];
  child: ChildProcess;
  sink: Sink;
  sandbox: string;
  home: string;
  dataBaseDir: string;
}

/**
 * 起一次真 webview 往返：构建（幂等）→ sink → harness bin → 等页面报告。
 *
 * `dataBaseDirIsFile` 是**失败路径**的造法：数据**基**目录的位置上放一个文件，真存储起来
 * （`mkdir {base}` / 打开 sqlite）必然 `ENOTDIR`，于是产品 Host 在就绪前退出，
 * 壳的 reaper 推 `failed(transport_closed)` —— 整条失败链是产品件，没有替身。
 */
async function runHarness(options: { dataBaseDirIsFile?: boolean } = {}): Promise<HarnessRun> {
  const sandbox = mkdtempSync(join(tmpdir(), "mode-host-webview-"));
  const home = join(sandbox, "home");
  const dataBaseDir = join(sandbox, "data");
  if (options.dataBaseDirIsFile) writeFileSync(dataBaseDir, "这不是目录");

  const sink = await startSink();
  const child = spawn(harnessBin, [], {
    cwd: appDir,
    env: {
      ...process.env,
      MODE_HOST_SIDECAR_ENTRY: resolve(repoRoot, "packages/desktop/src/host-standalone/index.ts"),
      MODE_DATA_BASE_DIR: dataBaseDir,
      HOME: home,
      USERPROFILE: home,
      MODE_HOST_ROUNDTRIP_TIMEOUT_MS: String(ROUNDTRIP_TIMEOUT_MS),
    },
    stdio: ["ignore", "inherit", "inherit"],
  });

  const deadline = Date.now() + REPORT_TIMEOUT_MS;
  while (sink.reports.length === 0 && Date.now() < deadline) {
    await delay(250);
  }
  if (sink.reports.length === 0) {
    child.kill();
    await closeSink(sink);
    throw new Error(`页面没有报告；页面日志：\n${sink.lines.join("\n")}`);
  }
  return { report: sink.reports[0]!, lines: sink.lines, child, sink, sandbox, home, dataBaseDir };
}

/** 关 sink（幂等）。**不关会让整轮测试挂住**——理由见 `finishRun`。 */
async function closeSink(sink: Sink): Promise<void> {
  await new Promise<void>((resolveClose) => {
    sink.server.close(() => resolveClose());
    setTimeout(resolveClose, 2_000);
  });
}

/**
 * 收摊：等壳自己退（看门狗 → `RunEvent::Exit` 收 Host），再关 sink 与沙箱。
 *
 * **sink 必须关**：它是个监听端口的 HTTP server，不关就握着句柄不放，`node --test` 会等这个
 * 文件的事件循环排空——表现为「用例全绿但整套测试挂住不退出」（本任务实测踩到过一次）。
 */
async function finishRun(run: HarnessRun): Promise<void> {
  await waitForExit(run.child, 60_000);
  if (run.child.exitCode === null) run.child.kill();
  await delay(500);
  await closeSink(run.sink);
  rmSync(run.sandbox, { recursive: true, force: true });
}

/** 断言某个 pid 不在进程表里（壳正常退出后不得残留 Host）。 */
function assertNoProcess(pid: number, what: string): void {
  const tasklist = spawnSync("tasklist", ["/FI", `PID eq ${pid}`, "/NH", "/FO", "CSV"], {
    encoding: "utf8",
  });
  assert.ok(!tasklist.stdout.includes(`"${pid}"`), `${what}（pid ${pid}）`);
}

test(
  "真 webview（成功路径）：产品壳 + 产品 Host 让渲染层的启动闸门放行，且退出不留孤儿",
  { skip: enabled ? false : "opt-in：设 MODE_TAURI_WEBVIEW_ROUNDTRIP=1 才跑（要真 webview）" },
  async () => {
    // 顺序不能倒：`frontendDist` 由 `tauri::generate_context!()` 在**编译期**内嵌，
    // 先编 Rust 会把页面漏在外面（Task 0 的 spike §3.2 实测过这个坑）。两条用例共用一次构建。
    buildFrontend(resolve(appDir, "vite.config.ts"));
    buildFrontend(resolve(appDir, "test/harness/host-sidecar/vite.config.ts"));
    buildHarnessBin();

    const run = await runHarness();
    try {
      const report = run.report;
      assert.equal(report.status, "ok", `页面报告失败：${report.message ?? JSON.stringify(report)}`);
      assert.equal(report.gateOpened, true, "渲染层未改动的准入闸门交出了端口");
      assert.equal(report.portIsRealMessagePort, true, "端口是真 MessagePort");
      assert.equal(report.initializeObserved, true, "端口承载了 Host 的 Initialize");
      assert.equal(report.schemaAccepted, true, "状态通过真 databaseStartupStateSchema");
      assert.equal(report.statePhase, "ready", "壳推给渲染层的状态是 ready");
      assert.equal(report.startupIdMatches, true, "端口负载的 databaseStartupId 与状态 startupId 同代");
      assert.equal(report.describe?.name, "mode-host-sidecar", "端口背后是产品 Host");
      // `MODE_DATA_BASE_DIR` 是**基**目录：根 = {base}/.mode（口径同 packages/services/src/paths.ts）。
      assert.equal(report.describe?.dataBaseDir, run.dataBaseDir, "数据隔离生效（覆写的基目录）");
      assert.equal(
        report.describe?.dataRoot,
        join(run.dataBaseDir, ".mode"),
        "数据根 = {base}/.mode（不是把基目录当根，见 2026-10-16 复评 I2）",
      );
      assert.ok(
        (report.describe?.serviceChannels?.length ?? 0) >= 40,
        "Host 暴露了完整（真）服务面",
      );
      // Task 2：隔离目录里**出现了**真数据根 —— 服务面是真的，闸门放行的不是空壳。
      assert.equal(
        existsSync(join(run.dataBaseDir, ".mode")),
        true,
        "真服务必须把数据写进 {base}/.mode（Task 2 起 Host 真写盘）",
      );

      await finishRun(run);
      const hostPid = report.describe?.pid;
      assert.ok(typeof hostPid === "number" && hostPid > 0, "页面报告应带上 Host pid");
      assertNoProcess(hostPid, "壳正常退出后不得残留 Host 进程");
    } finally {
      if (run.child.exitCode === null) run.child.kill();
      await closeSink(run.sink);
      rmSync(run.sandbox, { recursive: true, force: true });
    }
  },
);

test(
  "真 webview（失败路径）：真启动失败时页面收到 failed + errorCode，闸门不放行",
  { skip: enabled ? false : "opt-in：设 MODE_TAURI_WEBVIEW_ROUNDTRIP=1 才跑（要真 webview）" },
  async () => {
    buildHarnessBin();
    const run = await runHarness({ dataBaseDirIsFile: true });
    try {
      const report = run.report;
      console.log(`ROUNDTRIP_FAILURE_REPORT ${JSON.stringify(report)}`);
      // ① 失败必须**到达页面**：渲染层的启动计时器一收到状态就被拆除，所以「壳推 failed」
      //    是它此后唯一的失败信号（DR-T1-3）。页面没收到 = 渲染层会永远停在 starting。
      assert.equal(
        report.status,
        "startup_failed",
        `页面必须报告失败的启动状态：${JSON.stringify(report)}\n日志：\n${run.lines.join("\n")}`,
      );
      assert.equal(report.statePhase, "failed");
      assert.equal(report.schemaAccepted, true, "failed 状态必须过真 schema（否则渲染层整条丢掉）");
      // ② 错码必须是渲染层词汇表里的值，且落在失败发生的那一步上。
      //    `schemaAccepted: true` 已经证明整条状态（含 errorCode）过了页面里那份**真**
      //    `databaseStartupStateSchema`（`.strict()`）；这里再钉住具体取值，防止「合法但报错错了」。
      assert.equal(report.schemaAccepted, true, "failed 状态必须过真 schema（否则渲染层整条丢掉）");
      // 壳的 reaper 报的是「就绪前退出/传输断开」这一类（Host 在发第一帧之前就死了）。
      assert.equal(report.errorCode, "transport_closed");
      assert.equal(report.failedPhase, "starting", "失败发生在「拉起 Host」这一段");
      // ③ 闸门**不许**放行：失败就是失败，不能给出「挂上了但什么都不工作」的形态。
      assert.equal(report.gateOpened, false, "启动失败时闸门不得放行");
      assert.equal(report.initializeObserved, false, "失败路径下不该有 Initialize");
      // ④ 同代判据仍然成立（端口负载的 databaseStartupId 与失败状态的 startupId 同代），
      //    否则渲染层会把这条 failed 当作陈旧状态丢掉。
      assert.equal(report.startupIdMatches, true, "failed 状态与注入桥的 startupId 必须同代");
    } finally {
      await finishRun(run);
    }
  },
);
