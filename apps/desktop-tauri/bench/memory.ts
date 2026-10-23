/**
 * 常驻内存基线（M4a 性能四件之二，口径见路线图「度量与预算」节）。
 *
 * ## 测量面（混用即无效）
 *
 * 产品壳跑起来之后，**三个常驻进程**的 RSS 曲线：
 * - 壳（`ModeTauri.exe`，含 renderer 的 WebView2 子进程归属壳目录取并集）；
 * - sidecar Node（安装目录 `node/` 下的 node.exe，命令行含 host-sidecar bundle）；
 * - agent Node（安装目录 `node/` 下的 node.exe，命令行含 `agent/…/mode.cjs`）。
 *
 * 不在测量面内（如实声明）：
 * - CUA helper（懒启动，启动即零常驻是刻意设计，见 `services/node.ts` 的 Helper 懒启动注释）；
 * - WebView2 的 GPU / utility 子进程（按可执行名归属判定，见下）；
 * - 启动瞬态峰值——本脚本报的是**采样区间**的 min / 中位 / max，区间外不补测。
 *
 * ## 统计口径（先于数字写定）
 *
 * - 采样源：`Get-CimInstance Win32_Process` 的 `WorkingSetSize`（每进程 RSS）；
 * - 采样间隔默认 5 s；时长默认 30 min（路线图口径），`--duration <秒>` 可覆盖——
 *   **短跑只验证脚本正确性，不构成基线**；30 min 正式基线单独跑并抄进路线图；
 * - 壳进程按「安装目录前缀」归属（含 WebView2 子进程 `ModeTauri.exe` 同名派生）；
 *   sidecar / agent 按命令行特征区分（bundle 路径 / `mode.cjs`）；
 * - 必须 `--exe <安装目录里的 ModeTauri.exe>` 指定被测安装件，不猜默认路径；
 * - **前置**：被测机器上不能有正在运行的同名实例（产品壳带单实例守卫，新实例会静默
 *   秒退）；脚本会先检查并明确报错。用户的在跑会话属于用户，bench 不代杀。
 *
 * ```bash
 * # 短跑冒烟（90 秒，验证脚本正确性）
 * pnpm --filter @mode/desktop-tauri measure:memory -- --exe "C:/Users/.../ModeTauri.exe" --duration 90
 * # 正式基线（30 分钟，路线图口径）
 * pnpm --filter @mode/desktop-tauri measure:memory -- --exe "C:/Users/.../ModeTauri.exe"
 * ```
 */
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

import { runProductShell } from "../scripts/shell-process-probe.ts";

const DEFAULT_DURATION_SECONDS = 30 * 60;
const DEFAULT_INTERVAL_MS = 5_000;
const READY_MARK = { key: "ready", needle: "[mode-host-sidecar] ready startupId=" } as const;

type ProcessKind = "shell" | "sidecar" | "agent";

interface SampleRow {
  atMs: number;
  kind: ProcessKind;
  pid: number;
  workingSetBytes: number;
}

interface KindStats {
  kind: ProcessKind;
  samples: number;
  minBytes: number;
  medianBytes: number;
  maxBytes: number;
  finalBytes: number;
  pids: number[];
}

interface ProcessRow {
  ProcessId?: unknown;
  ExecutablePath?: unknown;
  CommandLine?: unknown;
  WorkingSetSize?: unknown;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
}

/** 一次性快照：候选进程的 pid/路径/命令行/RSS。
 *
 * **必须按 Name 过滤**（实测教训）：全量 `Get-CimInstance Win32_Process` 输出几百个进程的
 * CommandLine，stdout 超限被截断 → JSON 解析失败 → 静默空采样（2026-10-23 冒烟实测踩到）。
 * 过滤只减量；归属判定仍在 Node 侧的 classify() 里做，不在查询里预判。
 */
function snapshotProcesses(): ProcessRow[] {
  const script =
    "Get-CimInstance Win32_Process -Filter \"Name='ModeTauri.exe' OR Name='node.exe'\" | " +
    "Select-Object ProcessId,ExecutablePath,CommandLine,WorkingSetSize | ConvertTo-Json -Compress";
  const result = spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], {
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  const raw = String(result.stdout).trim();
  if (raw.length === 0) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  return (Array.isArray(parsed) ? parsed : [parsed]) as ProcessRow[];
}

/** 按安装目录与命令行特征把进程归类；不归属的（utility/GPU 等）返回 null 不计入。 */
function classify(row: ProcessRow, installDir: string): { kind: ProcessKind; pid: number; rss: number } | null {
  if (typeof row.ExecutablePath !== "string" || typeof row.WorkingSetSize !== "number") return null;
  const path = row.ExecutablePath.toLowerCase();
  const command = typeof row.CommandLine === "string" ? row.CommandLine.toLowerCase() : "";
  const pid = Number(row.ProcessId);
  const rss = Number(row.WorkingSetSize);
  if (!Number.isFinite(pid) || !Number.isFinite(rss)) return null;
  const prefix = installDir.toLowerCase();
  if (path.startsWith(prefix) && path.endsWith("modetauri.exe")) {
    return { kind: "shell", pid, rss };
  }
  // node.exe 必须在安装目录的 node/ 运行时下——不误收用户自己装的全局 node。
  if (path.startsWith(`${prefix}node\\`) && path.endsWith("node.exe")) {
    if (command.includes("host-sidecar") || command.includes("host-standalone")) {
      return { kind: "sidecar", pid, rss };
    }
    if (command.includes("mode.cjs") || command.includes(`${prefix}agent`)) {
      return { kind: "agent", pid, rss };
    }
  }
  return null;
}

function median(values: number[]): number {
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? ((sorted[middle - 1] as number) + (sorted[middle] as number)) / 2
    : (sorted[middle] as number);
}

function mib(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

function summarize(rows: SampleRow[]): KindStats[] {
  const byKind = new Map<ProcessKind, SampleRow[]>();
  for (const row of rows) {
    const list = byKind.get(row.kind) ?? [];
    list.push(row);
    byKind.set(row.kind, list);
  }
  const result: KindStats[] = [];
  for (const kind of ["shell", "sidecar", "agent"] as const) {
    const list = byKind.get(kind);
    if (!list || list.length === 0) continue;
    const values = list.map((row) => row.workingSetBytes);
    result.push({
      kind,
      samples: list.length,
      minBytes: Math.min(...values),
      medianBytes: median(values),
      maxBytes: Math.max(...values),
      finalBytes: values[values.length - 1] as number,
      pids: [...new Set(list.map((row) => row.pid))],
    });
  }
  return result;
}

/** 起壳前的单实例检查：产品壳带单实例守卫，已有实例时新实例会静默秒退（exit 0、零输出）， */
/** 等 ready 只会白等 120 s 再给一个含糊失败——所以在这里显式拦住并说清怎么办。 */
function assertNoRunningInstance(exe: string): void {
  const exeName = exe.split(/[\\/]/u).pop() ?? "";
  if (exeName.length === 0) return;
  const script =
    `Get-Process -Name "${exeName.replace(/\.exe$/iu, "")}" -ErrorAction SilentlyContinue | ` +
    "Select-Object Id | ConvertTo-Json -Compress";
  const result = spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], {
    encoding: "utf8",
  });
  const raw = String(result.stdout).trim();
  if (raw.length === 0) return;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return;
  }
  const rows = (Array.isArray(parsed) ? parsed : [parsed]) as { Id?: unknown }[];
  const pids = rows.map((row) => Number(row.Id)).filter((pid) => Number.isFinite(pid));
  if (pids.length > 0) {
    throw new Error(
      `已有 ${exeName} 实例在运行（pid ${pids.join(", ")}）——产品壳带单实例守卫，新实例会静默秒退。` +
        "请先退出正在运行的 Mode（它的会话属于用户，bench 不代杀），再跑本脚本。",
    );
  }
}

function parseArgs(argv: string[]): { exe: string; durationSeconds: number } {
  const exeIndex = argv.indexOf("--exe");
  if (exeIndex < 0 || argv[exeIndex + 1] === undefined) {
    throw new Error('需要 --exe <安装目录里的 ModeTauri.exe>（不猜默认路径，被测安装件必须显式指定）');
  }
  const exe = resolve(argv[exeIndex + 1] as string);
  const durationIndex = argv.indexOf("--duration");
  const durationSeconds =
    durationIndex >= 0 && argv[durationIndex + 1] !== undefined
      ? Number(argv[durationIndex + 1])
      : DEFAULT_DURATION_SECONDS;
  if (!Number.isFinite(durationSeconds) || durationSeconds <= 0) {
    throw new Error("--duration 必须是正数秒");
  }
  return { exe, durationSeconds };
}

async function main(): Promise<void> {
  if (process.platform !== "win32") {
    throw new Error("当前只支持 Windows（采样依赖 Win32_Process）；其他平台待补采集器后再测");
  }
  const { exe, durationSeconds } = parseArgs(process.argv.slice(2));
  const installDir = `${dirname(exe)}\\`;
  assertNoRunningInstance(exe);
  const isBaselineRun = durationSeconds >= DEFAULT_DURATION_SECONDS;
  console.log(
    `[memory] 被测：${exe}\n` +
      `[memory] 时长 ${String(durationSeconds)} s，采样间隔 ${DEFAULT_INTERVAL_MS / 1000} s` +
      (isBaselineRun
        ? "（路线图正式基线口径）"
        : "（短跑冒烟：只验证脚本正确性，**不构成基线**；正式基线跑满 30 min）"),
  );

  const samples: SampleRow[] = [];
  let sampling = true;
  const sampler = (async () => {
    const started = Date.now();
    while (sampling) {
      const atMs = Date.now() - started;
      for (const row of snapshotProcesses()) {
        const classified = classify(row, installDir);
        if (classified) {
          samples.push({
            atMs,
            kind: classified.kind,
            pid: classified.pid,
            workingSetBytes: classified.rss,
          });
        }
      }
      await sleep(DEFAULT_INTERVAL_MS);
    }
  })();

  // 起产品壳：等 host ready 后按 settle 时长保持运行（采样并发进行），probe 收尾走正常退出路径。
  // 数据根与 HOME 放临时目录：不污染真实用户数据，也不往安装目录写（那会污染后续采样的归属判定）。
  const workRoot = join(tmpdir(), `mode-memory-bench-${String(process.pid)}`);
  const trace = await runProductShell({
    exe,
    dataBaseDir: join(workRoot, "data"),
    home: join(workRoot, "home"),
    marks: [READY_MARK],
    timeoutMs: 120_000,
    settleMs: durationSeconds * 1000,
  });
  sampling = false;
  await sampler;

  if (trace.marks[READY_MARK.key] === undefined) {
    // 失败必须带证据：把 stderr 尾部打出来，否则「没起来」与「起来了但格式变了」无法区分。
    const tail = trace.lines
      .filter((line) => line.stream === "err")
      .slice(-30)
      .map((line) => `  +${String(line.at)}ms ${line.text}`)
      .join("\n");
    throw new Error(
      `Host ready 标记未出现（收尾=${trace.closedBy}，退出码=${String(trace.exitCode)}）——` +
        `壳没起来、Host 失败、或该安装件的标记格式不同。stderr 尾部：\n${tail || "（无 stderr 输出）"}`,
    );
  }

  // 收尾残留检查：正常退出路径下三类进程应全部消失；留着就是泄漏，必须显式警告。
  await sleep(2_000);
  const leftover = snapshotProcesses()
    .map((row) => classify(row, installDir))
    .filter((entry): entry is { kind: ProcessKind; pid: number; rss: number } => entry !== null);
  if (leftover.length > 0) {
    console.warn(
      `[memory] 警告：收尾后仍有归属进程未退出——${leftover
        .map((entry) => `${entry.kind}#${String(entry.pid)}`)
        .join(", ")}（正常退出路径不该留孤儿，见路线图「Host 崩溃/退出不留孤儿会话」）`,
    );
  }

  const stats = summarize(samples);
  if (stats.length === 0) {
    throw new Error("一次采样都没归属到三类进程——安装目录或命令行特征变了，先修 classify()");
  }
  console.log(`[memory] 采样 ${String(samples.length)} 行（含同进程多次），收尾=${trace.closedBy}`);
  for (const stat of stats) {
    console.log(
      `[memory] ${stat.kind}（出现过的 pid：${stat.pids.join(", ")}）：` +
        `min ${mib(stat.minBytes)} / 中位 ${mib(stat.medianBytes)} / max ${mib(stat.maxBytes)} / ` +
        `末值 ${mib(stat.finalBytes)}（n=${String(stat.samples)}）`,
    );
  }
  const totalMedian = stats.reduce((sum, stat) => sum + stat.medianBytes, 0);
  console.log(`[memory] 三类进程中位和：${mib(totalMedian)}（对照目标「会话空闲 RSS ≤150 MB」）`);
}

main().catch((error: unknown) => {
  console.error(`[memory] 失败：${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
