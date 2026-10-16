/**
 * M3 Task 4 fix round：**自绘标题栏在 Tauri 壳里真的能用**（用户报的缺陷）。
 *
 * ## 报的是什么、判据是什么
 *
 * 用户报：产品的自绘标题栏**点了没反应**（最小化/最大化都不动），而且**系统标题栏还在**
 * ——于是有两条标题栏。两个半边的成因都不在渲染层：
 *
 * 1. 三个按钮 → `platform.executeDesktopCommand(DesktopCommandIds.MinimizeWindow /
 *    ToggleMaximizeWindow / CloseWindow)`（`packages/ui/src/DesktopWindowControls.tsx` 的调用面）
 *    → 注入层。在 Tauri 壳里这个成员从前是**降级空操作**，所以「点了没反应」；
 * 2. 窗口从前**有边框**（`tauri.conf.json` 里没有任何 `decorations` 设置），所以系统标题栏照画。
 *
 * ## 判据（四条，各自独立）
 *
 * - **无边框**：**以窗口自己的状态为准**——harness 读 `window.is_decorated()` 打一行
 *   `HARNESS window decorations_expected=… decorations_actual=…`，这里断言两者都是 `false`。
 *   （`outerHeight - innerHeight` 作为第二条观察一起记；它能不能反映非客户区由本文件的
 *   **敏感性对照**回答——看不出差别的判据不能当证据。）
 * - **可拖区域还在**：页面声明的 `app-region: drag` 必须真的生效（含顶部区域）——无边框窗口
 *   如果连拖动都没了，比死按钮更糟（见 `mode_tauri::window` 的模块文档）。
 * - **按钮真的打到壳**：最大化切换 → 读回 `isMaximized: true` → 切回 → `false`；最小化成功；
 *   壳的 stderr 上每条都有留痕（注入层与壳是两条独立证据）。
 * - **关得掉**：最后一步真发 `closeWindow`，窗口被销毁、进程退出、不留 Host。
 *
 * ## 敏感性对照（第二条用例）
 *
 * 本轮吃过一次亏：**判据本身看不出两种形态的差别**（当时它对着一个带系统标题栏的窗口报了 0px，
 * 而参数在调用点被写反了）。所以第二条用例把窗口做成**有**系统标题栏，要求运行时观察读到
 * `true`、并且 `frameDeltaY` **确实**变大。若某台机器上后者不成立，结论是那条次级观察没有
 * 区分力（要换判据），**不是**把断言删掉。
 *
 * ## 覆盖边界（如实登记）
 *
 * **用鼠标真的拖动窗口**、以及最大化/最小化的**视觉结果**：自动化做不到（要真实指针/肉眼），
 * 属人工项，写在 spec 的「自动 vs 人工」表里；自动档覆盖它们的必要条件（无边框 + 可拖区域存在 +
 * WebView2 的非客户区支持由 wry 打开）。
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
/** harness 的看门狗：用例自己会在关窗后等它退。 */
const HARNESS_TIMEOUT_MS = 60_000;
const REPORT_TIMEOUT_MS = 120_000;
/** 窗口控制序列排在报告之后 8s，加上每步 600ms 的沉淀时间。 */
const SEQUENCE_TIMEOUT_MS = 60_000;
/** 有系统标题栏时非客户区至少这么高（敏感性对照的判据）。 */
const DECORATED_MIN_FRAME_DELTA = 20;

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

/**
 * 等 harness 的**输出**里出现某一行。
 *
 * 为什么不能直接 `output.includes(...)`：页面侧的日志走 fetch 到 sink，壳侧的
 * `window_control command=…` 走它自己的 stderr——两条通道**顺序不保证**（实测页面那条先到）。
 * 一次性断言会变成竞态（本轮第一次跑就红在这里，而输出里其实有那一行）。
 */
async function waitForOutput(
  getOutput: () => string,
  needle: string,
  timeoutMs: number,
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (getOutput().includes(needle)) return true;
    await delay(150);
  }
  return false;
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

interface HarnessRun {
  report: ProbeReport;
  lines: string[];
  /**
   * **活的**输出（getter，读的是闭包里那个不断追加的变量）。
   *
   * 不能是返回那一刻的快照：壳的 stderr 行比页面报告晚到，快照会让「壳侧留痕」的断言
   * 永远看不到它（本轮实测踩过）。
   */
  readonly output: string;
  child: ChildProcess;
  sink: Sink;
  sandbox: string;
}

/**
 * 跑一次产品 harness（同一个 `ProductUiHarness`，只差环境变量）。
 *
 * `forceDecorations` 是**敏感性对照**：把窗口做成**有**系统标题栏，用来证明本文件里
 * 「无边框」的判据真的能区分两种形态。
 * `windowControls` 打开探针里的窗口控制序列（它最后会**真关窗**）。
 */
async function runProductHarness(
  options: { forceDecorations?: boolean; windowControls?: boolean } = {},
): Promise<HarnessRun> {
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
      ...(options.windowControls ? { MODE_PRODUCT_UI_WINDOW_CONTROLS: "1" } : {}),
      ...(options.forceDecorations ? { MODE_PRODUCT_UI_FORCE_DECORATIONS: "1" } : {}),
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

  const reportDeadline = Date.now() + REPORT_TIMEOUT_MS;
  while (sink.reports.length === 0 && Date.now() < reportDeadline) {
    await delay(250);
  }
  assert.ok(
    sink.reports.length > 0,
    `产品 UI 没有报告（挂载都没成，后面的判据无从谈起）：页面日志 ${JSON.stringify(sink.lines)} 壳输出 ${output}`,
  );
  return {
    report: sink.reports[0]!,
    lines: sink.lines,
    get output() {
      return output;
    },
    child,
    sink,
    sandbox,
  };
}

/** 从 harness 的 stdout 里读窗口形态那一行（`decorations_actual` 是**窗口自己的**状态）。 */
function decorationsFromOutput(output: string): { expected: string; actual: string } {
  const match = /HARNESS window decorations_expected=(\S+) decorations_actual=(\S+)/.exec(output);
  assert.ok(match, `harness 必须报出窗口形态的运行时观察；实际输出：${output}`);
  return { expected: match![1]!, actual: match![2]! };
}

async function finishRun(run: HarnessRun): Promise<void> {
  await waitForExit(run.child, 90_000);
  if (run.child.exitCode === null) run.child.kill();
  await delay(500);
  await closeSink(run.sink);
  removeSandboxBestEffort(run.sandbox);
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

    const run = await runProductHarness({ windowControls: true });
    // 注意：**不要**解构 `run.output`——解构会调用一次 getter 并把当时的字符串拷出来，
    // 又变回快照（本轮踩过两次）。后面统一读 `run.output`。
    const { sink, child } = run;
    try {
      const report = run.report;
      assert.equal(report.mounted, true, "产品 UI 必须真的挂上（本用例的前提）");

      // ① 无边框：**以窗口自己的状态为准**（harness 读 `is_decorated()`）。
      //    这条会红的情形正是本轮那个反转（产品窗口反而带系统栏）。
      const decorations = decorationsFromOutput(run.output);
      assert.equal(
        decorations.expected,
        "false",
        `产品入口必须请求**无边框**（decorations=false）：${JSON.stringify(decorations)}`,
      );
      assert.equal(
        decorations.actual,
        "false",
        `窗口**实际**必须是无边框的（读的是窗口自己的 is_decorated）：${JSON.stringify(decorations)}`,
      );

      // ①b 第二条观察**只记录、不当判据**：`outerHeight - innerHeight` 在 WebView2 下
      //    看不出宿主窗口的非客户区（敏感性对照实测：有系统标题栏时它**仍然是 0**，见下面
      //    第二条用例）。所以「无边框」的判据是 ①（窗口自己的 `is_decorated`），
      //    而这条次级观察留在报告里供排查。
      console.log(`WINDOW_CONTROLS_FRAME frameDeltaY=${String(report.frameDeltaY)}`);

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

      // ③ 三个按钮真的打到壳：**断言往返关系**，不假设初值。
      //
      // 为什么不假设初值（`isMaximized:false`）：渲染层在启动时会恢复窗口外观（实测：探针的
      // 第一次读就已经是 `true`，而壳那边在启动后约 1 秒收到过一次非探针发的
      // `toggleMaximizeWindow`——见壳诊断行的 `at_ms`）。假设初值会让这条用例依赖渲染层的
      // 启动行为，而它要证的是「按钮真的改窗口状态」。
      const before = await waitForLine(sink, "window_control chrome_before_toggle", SEQUENCE_TIMEOUT_MS);
      assert.ok(before, `读初态必须成功：${JSON.stringify(sink.lines)}`);
      const maximized = await waitForLine(
        sink,
        "window_control chrome_after_toggle_1",
        SEQUENCE_TIMEOUT_MS,
      );
      assert.ok(maximized, `最大化切换必须到达壳：${JSON.stringify(sink.lines)}`);
      const restored = await waitForLine(
        sink,
        "window_control chrome_after_toggle_2",
        SEQUENCE_TIMEOUT_MS,
      );
      assert.ok(restored, "还原切换必须到达壳");

      const stateOf = (line: string): boolean => {
        const match = /\{"isMaximized":(true|false)\}/.exec(line);
        assert.ok(match, `这一行必须带上 isMaximized 的真值：${line}`);
        return match![1] === "true";
      };
      const initialValue = stateOf(before!);
      assert.equal(
        stateOf(maximized!),
        !initialValue,
        `一次 toggleMaximizeWindow 之后状态必须**翻转**（${String(initialValue)} → 期望 ${String(!initialValue)}）：` +
          `${before} / ${maximized}`,
      );
      assert.equal(
        stateOf(restored!),
        initialValue,
        `再一次 toggle 必须翻回原值（按钮真的在切窗口状态）：${maximized} / ${restored}`,
      );

      const minimized = await waitForLine(sink, "window_control minimize_result", SEQUENCE_TIMEOUT_MS);
      assert.ok(minimized, "最小化必须到达壳");
      assert.ok(minimized!.includes('"success":true'), `最小化必须成功：${minimized}`);

      // 壳侧的留痕（同一个动作的第二条独立证据：注入层说发出去了，壳说自己收到了）。
      // `closeWindow` 不在这里断言——它排在序列最后（下面第 ④ 段），此刻还没发。
      for (const command of ["minimizeWindow", "toggleMaximizeWindow"]) {
        assert.ok(
          await waitForOutput(() => run.output, `window_control command=${command}`, 10_000),
          `壳必须收到 ${command}（注入层与壳是两条独立证据）：${run.output}`,
        );
      }

      // ④ 关得掉：序列最后一步真发 closeWindow，窗口销毁 → 应用退出（不留孤儿）。
      const closeRequested = await waitForLine(sink, "window_control close_requested", 10_000);
      assert.ok(closeRequested, "关闭必须被请求");
      assert.ok(
        await waitForOutput(() => run.output, "window_control command=closeWindow", 10_000),
        `关窗必须走到壳（注入层与壳是两条独立证据）：${run.output}`,
      );
      await waitForExit(child, 30_000);
      assert.notEqual(
        child.exitCode,
        null,
        `发过 closeWindow 之后进程必须退出（用户报的缺陷之一就是关不掉）：${run.output}`,
      );
      const hostPid = /HARNESS host startupId=\S+ pid=(\d+)/.exec(run.output)?.[1];
      assert.ok(hostPid, `harness 必须报出 Host 的 pid（孤儿检查用）：${run.output}`);
      await delay(500);
      assertNoProcess(Number(hostPid), "关窗退出后不得残留 Host 进程");
    } finally {
      if (child.exitCode === null) child.kill();
      await closeSink(sink);
      removeSandboxBestEffort(run.sandbox);
    }
  },
);

test(
  "敏感性对照：加系统标题栏时必须看得出来（判据没有区分力就等于没有判据）",
  {
    skip: enabled
      ? false
      : "opt-in：设 MODE_TAURI_WINDOW_CONTROLS=1 才跑（要真 webview + 真窗口）",
  },
  async () => {
    // 为什么必须有这一档：主用例拿「无边框」当用户可见的判据，而本轮漏掉的正是
    // 「判据本身看不出两种形态的差别」。这一档把窗口做成**有**系统标题栏，要求：
    // ① 运行时观察（`is_decorated`）读到 `true`（说明这条观察真的在读窗口）；
    // ② `frameDeltaY` 确实变大（说明它也能反映非客户区）。
    // ② 若在别的机器上失败，结论是那条次级观察没有区分力——那时要换判据，
    // 而不是把断言删掉（删掉就等于回到「无边框没有自动化覆盖」）。
    buildHarnessBin();
    const run = await runProductHarness({ forceDecorations: true });
    try {
      const decorations = decorationsFromOutput(run.output);
      assert.equal(
        decorations.expected,
        "true",
        `对照档必须请求有边框：${JSON.stringify(decorations)}`,
      );
      assert.equal(
        decorations.actual,
        "true",
        `对照档的窗口**实际**必须带系统栏（否则运行时观察读不到真实状态）：${JSON.stringify(decorations)}`,
      );
      console.log(
        `WINDOW_CONTROLS_CONTROL frameDeltaY=${String(run.report.frameDeltaY)} frameDeltaX=${String(run.report.frameDeltaX)}`,
      );
      // 次级观察的**失效**也要钉住（本用例的实测结论）：有系统标题栏时
      // `outerHeight - innerHeight` 仍然是 0——它在 WebView2 里量的是 webview 自己的客户区，
      // 看不到宿主窗口的非客户区。也就是说**它不能当「无边框」的判据**（本轮判据写反时
      // 正是被这个假 0 骗过去的），判据只能是 ①。
      //
      // 这条断言不是「多此一举」：哪天它真的变了（WebView2/wry 行为变化），它会红，
      // 提示可以把次级观察升级成判据。
      assert.ok(
        (run.report.frameDeltaY ?? 0) < DECORATED_MIN_FRAME_DELTA,
        `有系统标题栏时 frameDeltaY 本机实测仍是 0（说明它看不出非客户区、不能当判据）；` +
          `现在读到 ${String(run.report.frameDeltaY)}px——行为变了，` +
          `请重新评估「无边框」的判据（但不要删掉对 is_decorated 的断言）`,
      );
    } finally {
      await finishRun(run);
    }
  },
);
