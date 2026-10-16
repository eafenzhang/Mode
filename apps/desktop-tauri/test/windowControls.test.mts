/**
 * M3 Task 4 fix round：**自绘标题栏在 Tauri 壳里真的能用**（用户报的缺陷）。
 *
 * ## 报的是什么、判据是什么
 *
 * 用户报：产品的自绘标题栏**点了没反应**（最小化/最大化都不动），而且**系统标题栏还在**
 * ——于是有两条标题栏。两个半边的成因都不在渲染层：
 *
 * 1. 三个按钮 → `platform.executeDesktopCommand(DesktopCommandIds.MinimizeWindow | ToggleMaximizeWindow |
 *    CloseWindow)`（`packages/ui/src/DesktopWindowControls.tsx` 的调用面）→ 注入层。
 *    在 Tauri 壳里这个成员是**降级空操作**（必选、返回 `Promise<void>` 那一类），所以「点了没反应」；
 * 2. 窗口是**有边框**的（`tauri.conf.json` 里没有任何 `decorations` 设置），所以系统标题栏照画。
 *
 * 本文件在**真窗口**上断言四件事（各自独立，缺一条就不算修好）：
 *
 * - **无边框**：`outerHeight - innerHeight`（系统加的非客户区高度）必须≈0。有系统标题栏时
 *   它是标题栏+边框（本机实测 30+ px）；
 * - **可拖区域还在**：页面声明的 `app-region: drag` 区域必须真的有（含顶部区域）——
 *   无边框窗口如果连拖动都没了，比死按钮更糟（见 `mode_tauri::window` 的模块文档）；
 * - **按钮真的打到壳**：最大化切换 → 读回 `isMaximized: true` → 切回 → `false`；最小化发出成功；
 * - **关得掉**：最后一步真发 `closeWindow`，窗口被销毁、进程退出（不留孤儿）。
 *
 * ## 覆盖边界（如实登记）
 *
 * - **真的用鼠标拖动窗口**：自动化做不到（要真实指针输入）。可自动化的是它的**必要条件**
 *   ——「页面声明了 `app-region: drag` 区域」+「WebView2 的非客户区支持由 wry 打开」
 *   （`wry 0.57.0` 的 `setIsNonClientRegionSupportEnabled(true)`，见 spec）。**人手拖一次**
 *   是发版前的人工项，写在 spec 的「自动 vs 人工」表里；
 * - 最大化/还原/最小化的**视觉结果**（图标形状、任务栏状态）同理属人工项，这里断言的是
 *   窗口状态本身（`isMaximized` 真值 + 命令返回 + 进程退出）。
 *
 * ```bash
 * MODE_TAURI_WINDOW_CONTROLS=1 node --import tsx --test apps/desktop-tauri/test/windowControls.test.mts
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
/** 与其它 opt-in 文件共用（探针的上报地址在脚本里是固定的 127.0.0.1:5177）。 */
const SINK_PORT = 5177;
/** A 的看门狗：用例自己会在关窗后等它退；给足余量。 */
const HARNESS_TIMEOUT_MS = 60_000;
const REPORT_TIMEOUT_MS = 120_000;
/** 窗口控制序列排在报告之后 8s，加上每步 600ms 的沉淀时间。 */
const SEQUENCE_TIMEOUT_MS = 60_000;

const enabled = process.env.MODE_TAURI_WINDOW_CONTROLS === "1";

interface ProbeReport {
  status: string;
  mounted?: boolean;
  phase?: string | null;
  frameDeltaX?: number | null;
  frameDeltaY?: number | null;
  dragRegionCount?: number | null;
  topDragRegionCount?: number | null;
  noDragRegionCount?: number | null;
  degraded?: string[] | null;
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
        console.log(`WINDOW_CONTROLS_PAGE_LOG ${line}`);
      }
      if (report) {
        sink.reports.push(JSON.parse(report) as ProbeReport);
        console.log(`WINDOW_CONTROLS_REPORT ${report}`);
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

function assembleFrontend(): void {
  run(
    process.execPath,
    ["--import", "tsx", resolve(appDir, "scripts/build-renderer-for-tauri.ts"), "--no-bench"],
    repoRoot,
  );
  assert.ok(existsSync(join(appDir, "dist/index.html")), "装配后的 dist 里必须有产品页");
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

function removeSandboxBestEffort(sandbox: string): void {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      rmSync(sandbox, { recursive: true, force: true });
      return;
    } catch {
      spawnSync(process.execPath, ["-e", "setTimeout(function(){}, 300)"], { stdio: "ignore" });
    }
  }
  console.warn(`[window-controls] 沙箱未能删除（不影响断言）：${sandbox}`);
}

/** 等 sink 里出现某一行（窗口控制序列是异步的，逐步断言各自的出现）。 */
async function waitForLine(sink: Sink, needle: string, timeoutMs: number): Promise<string | null> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const found = sink.lines.find((line) => line.includes(needle));
    if (found) return found;
    await delay(200);
  }
  return null;
}

test(
  "自绘标题栏在真窗口上可用：无边框、可拖区域在、三个按钮真的打到壳、且关得掉",
  {
    skip: enabled
      ? false
      : "opt-in：设 MODE_TAURI_WINDOW_CONTROLS=1 才跑（要真 webview + 真窗口）",
  },
  async () => {
    assembleFrontend();
    buildHarnessBin();

    const sink = await startSink();
    const sandbox = mkdtempSync(join(tmpdir(), "mode-window-controls-"));
    const home = join(sandbox, "home");
    const dataBaseDir = join(sandbox, "data");
    let output = "";
    const child = spawn(harnessBin, [], {
      cwd: appDir,
      env: {
        ...process.env,
        MODE_HOST_SIDECAR_ENTRY: resolve(repoRoot, "packages/desktop/src/host-standalone/index.ts"),
        MODE_DATA_BASE_DIR: dataBaseDir,
        HOME: home,
        USERPROFILE: home,
        MODE_PRODUCT_UI_TIMEOUT_MS: String(HARNESS_TIMEOUT_MS),
        MODE_PRODUCT_UI_WINDOW_CONTROLS: "1",
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    child.stdout?.on("data", (chunk: Buffer) => {
      output += chunk.toString("utf8");
      process.stdout.write(chunk);
    });
    child.stderr?.on("data", (chunk: Buffer) => {
      output += chunk.toString("utf8");
      process.stderr.write(chunk);
    });

    try {
      const reportDeadline = Date.now() + REPORT_TIMEOUT_MS;
      while (sink.reports.length === 0 && Date.now() < reportDeadline) {
        await delay(250);
      }
      assert.ok(
        sink.reports.length > 0,
        `产品 UI 没有报告（挂载都没成，后面的判据无从谈起）：\n${sink.lines.join("\n")}`,
      );
      const report = sink.reports[0]!;
      assert.equal(report.mounted, true, "产品 UI 必须真的挂上（本用例的前提）");

      // ① 无边框：系统加的非客户区高度必须≈0（有系统标题栏时是 30+ px）。
      assert.ok(
        typeof report.frameDeltaY === "number",
        `探针必须报出窗口形态（frameDeltaY）：${JSON.stringify(report)}`,
      );
      assert.ok(
        report.frameDeltaY! < 20,
        `窗口必须是无边框的（系统标题栏就是这条高度；实测 ${report.frameDeltaY}px）——` +
          `否则用户会看到两条标题栏`,
      );

      // ② 可拖区域：页面声明的 `app-region: drag` 必须真的生效（含顶部区域）。
      //    无边框 + 没有可拖区域 = 一块移不动的板子（比死按钮更糟）。
      assert.ok(
        (report.dragRegionCount ?? 0) > 0,
        `页面必须声明可拖区域（computed style 里 app-region:drag），实际 ${report.dragRegionCount}`,
      );
      assert.ok(
        (report.topDragRegionCount ?? 0) > 0,
        `顶部必须有可拖区域（用户就拖那里），实际 ${report.topDragRegionCount}`,
      );
      assert.ok(
        (report.noDragRegionCount ?? 0) > 0,
        "按钮所在区域必须是 no-drag（否则点按钮会变成拖窗口）",
      );

      // ③ 三个按钮真的打到壳：每一步一条日志。
      const initial = await waitForLine(sink, "window_control chrome_initial", SEQUENCE_TIMEOUT_MS);
      assert.ok(initial, `读初态必须成功：\n${sink.lines.join("\n")}`);
      assert.ok(initial!.includes('"isMaximized":false'), `初态应当是非最大化：${initial}`);

      const maximized = await waitForLine(sink, "window_control chrome_maximized", SEQUENCE_TIMEOUT_MS);
      assert.ok(maximized, `最大化切换必须到达壳：\n${sink.lines.join("\n")}`);
      assert.ok(
        maximized!.includes('"isMaximized":true'),
        `切到最大化之后读回的状态必须是 isMaximized:true（这是「按钮真的生效」的判据）：${maximized}`,
      );

      const restored = await waitForLine(sink, "window_control chrome_restored", SEQUENCE_TIMEOUT_MS);
      assert.ok(restored, "还原切换必须到达壳");
      assert.ok(restored!.includes('"isMaximized":false'), `还原后必须回到非最大化：${restored}`);

      const minimized = await waitForLine(sink, "window_control minimize_result", SEQUENCE_TIMEOUT_MS);
      assert.ok(minimized, "最小化必须到达壳");
      assert.ok(minimized!.includes('"success":true'), `最小化必须成功：${minimized}`);

      // 壳侧的留痕（同一个动作的第二条独立证据：注入层说发出去了，壳说自己收到了）。
      // `closeWindow` 不在这里断言——它排在序列最后（下面第 ④ 段），此刻还没发。
      for (const command of ["minimizeWindow", "toggleMaximizeWindow"]) {
        assert.ok(
          output.includes(`window_control command=${command}`),
          `壳必须收到 ${command}（注入层与壳是两条独立证据）：\n${output}`,
        );
      }

      // ④ 关得掉：序列最后一步真发 closeWindow，窗口销毁 → 应用退出（不留孤儿）。
      const closeRequested = await waitForLine(sink, "window_control close_requested", 10_000);
      assert.ok(closeRequested, "关闭必须被请求");
      assert.ok(
        output.includes("window_control command=closeWindow"),
        `关窗必须走到壳（注入层与壳是两条独立证据）：
${output}`,
      );
      await waitForExit(child, 30_000);
      assert.notEqual(
        child.exitCode,
        null,
        `发过 closeWindow 之后进程必须退出（用户报的缺陷之一就是关不掉）：\n${output}`,
      );
      const hostPid = /HARNESS host startupId=\S+ pid=(\d+)/.exec(output)?.[1];
      assert.ok(hostPid, `harness 必须报出 Host 的 pid（孤儿检查用）：\n${output}`);
      await delay(500);
      assertNoProcess(Number(hostPid), "关窗退出后不得残留 Host 进程");
    } finally {
      if (child.exitCode === null) child.kill();
      await closeSink(sink);
      removeSandboxBestEffort(sandbox);
    }
  },
);
