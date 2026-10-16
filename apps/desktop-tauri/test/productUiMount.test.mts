/**
 * M3 Task 3 · Step 4：**真产品 UI** 在 Tauri 壳里挂载并跑通业务面（**opt-in**，要真 webview）。
 *
 * 与 `webviewRoundTrip.test.mts` 的分工：那个文件验「壳 + Host + 端口」这条**传输**链，
 * 页面是集成测试页（自己实现接收形状）；本文件验的是**产品本身**——加载
 * `packages/desktop/out/renderer` 的构建产物（renderer 源码零改动），并断言：
 *
 * 1. `window.mode` 在**页面脚本之前**就位（探针在 document-start 读到它）；
 * 2. 渲染层入口模块跑完、React 真的挂在 `#root` 上（不是「窗口里显示了什么」就算数）；
 * 3. **出方向**有帧被壳收下并写进 Host 的 stdin（`acceptedFrames > 0`）；
 * 4. 闸门放行之后**入方向还在增长**（`pumpFramesGrewAfterReady`）——Host 只对请求作答，
 *    所以这等于「渲染层的请求到了真服务并拿到应答」；
 * 5. 退出不留孤儿（壳 + Host 都不在进程表里）。
 *
 * 为什么默认 skip：这五条都要真 webview（注入时机、`postMessage` 转移、ACL/本地内容、
 * raw 响应字节通路），CI 里跑不了。这是**发版前人工跑一次**的硬要求（漏跑 = 产品 UI
 * 挂不上而流程说全绿），与 Task 1/2 的那条同性质。
 *
 * ```bash
 * MODE_TAURI_PRODUCT_UI=1 node --import tsx --test apps/desktop-tauri/test/productUiMount.test.mts
 * ```
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
const harnessBin = resolve(repoRoot, "target/debug/ProductUiHarness.exe");
const SINK_PORT = 5177;
const HARNESS_TIMEOUT_MS = 40_000;
const REPORT_TIMEOUT_MS = 120_000;

const enabled = process.env.MODE_TAURI_PRODUCT_UI === "1";

interface ProbeReport {
  status: string;
  modeAtDocumentStart?: string;
  modeKeyCountAtDocumentStart?: number;
  modeDiagnosticsAtDocumentStart?: {
    implemented?: number;
    notImplemented?: number;
    deliberatelyAbsent?: string[];
  } | null;
  probeError?: string | null;
  rendererStarted?: boolean;
  mounted?: boolean;
  startupId?: string | null;
  portDelivered?: boolean | null;
  phase?: string | null;
  pumpFrames?: number | null;
  sentFrames?: number | null;
  acceptedFrames?: number | null;
  readyFrameWatermark?: number | null;
  pumpFramesGrewAfterReady?: boolean;
  bridgeError?: string | null;
  degraded?: string[] | null;
  uiText?: string;
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
        console.log(`PRODUCT_UI_PAGE_LOG ${line}`);
      }
      if (report) {
        const parsed = JSON.parse(report) as ProbeReport;
        sink.reports.push(parsed);
        console.log(`PRODUCT_UI_REPORT ${report}`);
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

/** 装配前端根（真 renderer 产物 + 验收台）——它会先清 `dist/`，所以必须在 harness 构建之前跑。 */
function assembleFrontend(): void {
  run(
    process.execPath,
    [
      "--import",
      "tsx",
      resolve(appDir, "scripts/build-renderer-for-tauri.ts"),
      "--no-bench",
    ],
    repoRoot,
  );
  assert.ok(
    existsSync(join(appDir, "dist/index.html")),
    "装配后的 dist 里必须有产品页 index.html（renderer 的构建产物）",
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

function assertNoProcess(pid: number, what: string): void {
  const tasklist = spawnSync("tasklist", ["/FI", `PID eq ${pid}`, "/NH", "/FO", "CSV"], {
    encoding: "utf8",
  });
  assert.ok(!tasklist.stdout.includes(`"${pid}"`), `${what}（pid ${pid}）`);
}

test(
  "真产品 UI：renderer 产物在 Tauri 壳里挂载、window.mode 先于页面脚本、业务帧真的走通",
  { skip: enabled ? false : "opt-in：设 MODE_TAURI_PRODUCT_UI=1 才跑（要真 webview + 真 renderer 产物）" },
  async () => {
    // 顺序不能倒：`frontendDist` 由 `tauri::generate_context!()` 在**编译期**内嵌。
    assembleFrontend();
    buildHarnessBin();

    const sandbox = mkdtempSync(join(tmpdir(), "mode-product-ui-"));
    const home = join(sandbox, "home");
    const dataBaseDir = join(sandbox, "data");
    const sink = await startSink();
    let harnessOutput = "";
    const child = spawn(harnessBin, [], {
      cwd: appDir,
      env: {
        ...process.env,
        MODE_HOST_SIDECAR_ENTRY: resolve(repoRoot, "packages/desktop/src/host-standalone/index.ts"),
        MODE_DATA_BASE_DIR: dataBaseDir,
        HOME: home,
        USERPROFILE: home,
        MODE_PRODUCT_UI_TIMEOUT_MS: String(HARNESS_TIMEOUT_MS),
      },
      // stdout 要解析 Host 的 pid（「不留孤儿」的断言要用），所以 pipe 出来再转发给控制台。
      stdio: ["ignore", "pipe", "inherit"],
    });
    child.stdout?.on("data", (chunk: Buffer) => {
      harnessOutput += chunk.toString("utf8");
      process.stdout.write(chunk);
    });

    try {
      const deadline = Date.now() + REPORT_TIMEOUT_MS;
      while (sink.reports.length === 0 && Date.now() < deadline) {
        await delay(250);
      }
      assert.ok(
        sink.reports.length > 0,
        `产品 UI 页面没有报告；页面日志：\n${sink.lines.join("\n")}`,
      );
      const report = sink.reports[0]!;
      assert.equal(
        report.status,
        "ok",
        `真产品 UI 必须挂载成功：${JSON.stringify(report)}\n日志：\n${sink.lines.join("\n")}`,
      );

      // ① 顺序性质：注入的 `window.mode` 在**页面脚本之前**就在（探针是 document-start 注入）。
      assert.equal(
        report.modeAtDocumentStart,
        "object",
        "探针在 document-start 必须已经能读到 window.mode（DR-T3-1 的顺序性质）",
      );
      assert.ok(
        (report.modeKeyCountAtDocumentStart ?? 0) > 0,
        "注入的 window.mode 上必须有成员（空对象等于没注入）",
      );
      // 非 M1 能力按 §17 的口径注入（要读值的 → 被拒 Promise；没有要读的值的 → 留痕空操作），
      // 所以降级**被记录**是预期的；但**不得**出现真正坏掉的痕迹
      // （缺 Tauri 全局、缺事件 API、订阅失败、invoke 失败）——那说明壳或注入层坏了，不是缺能力。
      const broken = (report.degraded ?? []).filter(
        (entry) =>
          !entry.startsWith("not_implemented_noop:") &&
          !entry.startsWith("not_implemented_rejected:"),
      );
      assert.deepEqual(broken, [], `注入层只应记录「未实现成员的降级」，实际：${broken.join(", ")}`);

      // ② 渲染层真的挂上了（React 在 `#root` 里有子树）。
      assert.equal(report.rendererStarted, true, "renderer 入口模块必须开始执行");
      assert.equal(report.mounted, true, "React 必须挂到 #root 上（这是「真产品 UI」的下限）");
      assert.equal(report.phase, "ready", `启动闸门必须放行：phase=${String(report.phase)}`);
      assert.equal(report.portDelivered, true, "端口必须送达渲染层");

      // ③ 出方向：页面侧发出的帧被壳收下并写进了 Host 的 stdin。
      assert.ok(
        (report.acceptedFrames ?? 0) > 0,
        `渲染层必须在端口上发过帧且被壳接受：acceptedFrames=${String(report.acceptedFrames)}`,
      );
      // ④ 入方向在闸门放行之后仍在增长：Host 只对请求作答 → 这是「请求到了真服务并拿到应答」。
      assert.equal(
        report.pumpFramesGrewAfterReady,
        true,
        `放行后入方向必须继续有帧（业务应答）：watermark=${String(report.readyFrameWatermark)} pumpFrames=${String(report.pumpFrames)}`,
      );
      assert.ok(
        (report.uiText ?? "").length > 0,
        `渲染出的 UI 必须非空（原始输出会入仓）：${JSON.stringify(report.uiText)}`,
      );
      // ⑤ 渲染出来的必须是**产品界面**，不是错误边界画出来的错误屏：注入层任何同步抛出
      //    都会被错误边界变成一块错误区域——那正是本任务两轮修法的依据（spec §10.8/§10.9）。
      assert.ok(
        !(report.uiText ?? "").includes("not_implemented_until_m3"),
        `界面里不得出现注入层的错误屏：${JSON.stringify(report.uiText)}`,
      );
      assert.equal(report.bridgeError, null, "注入桥不得报错");

      // ⑤ 收摊：壳自己退（看门狗 → RunEvent::Exit 收 Host），且不留孤儿。
      await waitForExit(child, 60_000);
      if (child.exitCode === null) child.kill();
      await delay(500);
      const hostPid = /HARNESS host startupId=\S+ pid=(\d+)/.exec(harnessOutput)?.[1];
      assert.ok(hostPid, `harness 必须报出 Host 的 pid（用于孤儿检查）：\n${harnessOutput}`);
      assertNoProcess(Number(hostPid), "壳退出后不得残留 Host 进程");
    } finally {
      if (child.exitCode === null) child.kill();
      await closeSink(sink);
      removeSandboxBestEffort(sandbox);
    }
  },
);

/**
 * 尽力删掉沙箱：Windows 上刚退出的进程可能还握着文件句柄（实测过一次 `EPERM`）。
 * 清理失败**不能**把上面的断言结果盖掉——那会让「用例到底过没过」变成读不清的事。
 */
function removeSandboxBestEffort(sandbox: string): void {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      rmSync(sandbox, { recursive: true, force: true });
      return;
    } catch {
      spawnSync(process.execPath, ["-e", "setTimeout(function(){}, 300)"], { stdio: "ignore" });
    }
  }
  console.warn(`[product-ui] 沙箱未能删除（不影响断言）：${sandbox}`);
}
