/**
 * **opt-in** 集成测试：在真 webview 里验「产品壳 + 产品 Host 让渲染层的启动闸门放行」。
 *
 * 为什么默认 skip（DR-T1-2 的第二层）：这条路径要真 webview，CI 里跑不了——
 * `initialization_script` 的注入时机、`window.postMessage(payload, "*", [port])` 的转移与
 * `event.source === window`、ACL/本地内容、Tauri raw 响应的真实字节通路，四件事都只有真窗口能给。
 * Task 0 的 spike 已证明它们可行（`docs/plans/2026-10-16-m3-port-transport-spike.md` §3），
 * 本用例把它落到**产品件**上：桥来自 `crates/sidecar-host/src/bridge.ts`，Host 来自
 * `packages/desktop/src/host-standalone/index.ts`，页面用未改动的
 * `DatabaseStartupAdmission` 当闸门。
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
import { existsSync, mkdtempSync, rmSync } from "node:fs";
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
  describe?: { name?: string; pid?: number; dataRoot?: string; startupId?: string; serviceChannels?: string[] };
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

/** 用本机 cargo 包装器构建 opt-in harness bin。 */
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

test(
  "真 webview：产品壳 + 产品 Host 让渲染层的启动闸门放行，且退出不留孤儿",
  { skip: enabled ? false : "opt-in：设 MODE_TAURI_WEBVIEW_ROUNDTRIP=1 才跑（要真 webview）" },
  async () => {
    const sandbox = mkdtempSync(join(tmpdir(), "mode-host-webview-"));
    const home = join(sandbox, "home");
    const dataRoot = join(sandbox, "data");
    const sink = await startSink();
    let child: ChildProcess | undefined;
    try {
      // 顺序不能倒：`frontendDist` 由 `tauri::generate_context!()` 在**编译期**内嵌，
      // 先编 Rust 会把页面漏在外面（Task 0 的 spike §3.2 实测过这个坑）。
      buildFrontend(resolve(appDir, "vite.config.ts"));
      buildFrontend(resolve(appDir, "test/harness/host-sidecar/vite.config.ts"));
      buildHarnessBin();

      child = spawn(harnessBin, [], {
        cwd: appDir,
        env: {
          ...process.env,
          MODE_HOST_SIDECAR_ENTRY: resolve(repoRoot, "packages/desktop/src/host-standalone/index.ts"),
          MODE_DATA_BASE_DIR: dataRoot,
          HOME: home,
          USERPROFILE: home,
          MODE_HOST_ROUNDTRIP_TIMEOUT_MS: String(ROUNDTRIP_TIMEOUT_MS),
        },
        stdio: ["ignore", "inherit", "inherit"],
      });

      const deadline = Date.now() + 90_000;
      while (sink.reports.length === 0 && Date.now() < deadline) {
        await delay(250);
      }
      assert.ok(sink.reports.length > 0, `90s 内没有收到页面报告；页面日志：\n${sink.lines.join("\n")}`);

      const report = sink.reports[0]!;
      assert.equal(report.status, "ok", `页面报告失败：${report.message ?? JSON.stringify(report)}`);
      assert.equal(report.gateOpened, true, "渲染层未改动的准入闸门交出了端口");
      assert.equal(report.portIsRealMessagePort, true, "端口是真 MessagePort");
      assert.equal(report.initializeObserved, true, "端口承载了 Host 的 Initialize");
      assert.equal(report.schemaAccepted, true, "状态通过真 databaseStartupStateSchema");
      assert.equal(report.statePhase, "ready", "壳推给渲染层的状态是 ready");
      assert.equal(report.startupIdMatches, true, "端口负载的 databaseStartupId 与状态 startupId 同代");
      assert.equal(report.describe?.name, "mode-host-sidecar", "端口背后是产品 Host");
      assert.equal(report.describe?.dataRoot, dataRoot, "数据隔离生效（MODE_DATA_BASE_DIR 覆写）");
      assert.ok(
        (report.describe?.serviceChannels?.length ?? 0) >= 40,
        "Host 暴露了完整（stub）服务面",
      );

      // 壳的看门狗到点会主动退出 → RunEvent::Exit 收 Host。等它退完再查进程表。
      if (child) await waitForExit(child, 60_000);
      const hostPid = report.describe?.pid;
      assert.ok(typeof hostPid === "number" && hostPid > 0, "页面报告应带上 Host pid");
      const tasklist = spawnSync("tasklist", ["/FI", `PID eq ${hostPid}`, "/NH", "/FO", "CSV"], {
        encoding: "utf8",
      });
      assert.ok(
        !tasklist.stdout.includes(`"${hostPid}"`),
        `壳正常退出后不得残留 Host 进程（pid ${hostPid}）`,
      );

      assert.equal(
        existsSync(join(home, ".mode")),
        false,
        "隔离生效：临时家目录里不得出现 .mode（更不得碰用户真实数据根）",
      );
    } finally {
      if (child && child.exitCode === null) child.kill();
      sink.server.close();
      rmSync(sandbox, { recursive: true, force: true });
    }
  },
);
