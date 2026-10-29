/**
 * Host 启动分段（M4b 归因尺子）。
 *
 * 从 Host 自身的日志里读 `startup_mark phase=<p> elapsedMs=<n>`（由
 * `packages/desktop/src/host-standalone/index.ts` 写出），按一次启动打印各段耗时。
 *
 * **口径**：`elapsedMs` 从 Host 模块初始化起算，所以 `native_deps` 那一段包含
 * Node 启动与模块装载；`T_ready` 的权威口径仍是壳收到 `ready startupId=` 那一行
 * （spec `tauri-renderer-host.md` §13.3），本脚本只做**分解**，不改口径。
 *
 * ```bash
 * pnpm measure:host-startup                  # 读今天最新的 host-YYYY-MM-DD.log
 * pnpm measure:host-startup --log <path>     # 指定日志
 * ```
 */
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export interface StartupMark {
  phase: string;
  elapsedMs: number;
}

const MARK_PATTERN = /\[mode-host-sidecar\] startup_mark phase=(\S+) elapsedMs=(\d+)/u;

/**
 * 解析一次启动的分段。
 *
 * 同一天日志里可能有多次启动：遇到 `native_deps` 视为新一次启动并重置，最终返回最后一组。
 */
export function parseStartupMarks(text: string): StartupMark[] {
  const marks: StartupMark[] = [];
  for (const line of text.split(/\r?\n/u)) {
    const match = MARK_PATTERN.exec(line);
    if (!match) continue;
    const phase = match[1];
    const elapsedMs = Number(match[2]);
    if (phase === "native_deps") marks.length = 0;
    marks.push({ phase, elapsedMs });
  }
  return marks;
}

/** 默认数据根下的 `v2/logs/host-YYYY-MM-DD.log`，取当天最新的一份。 */
export function findLatestHostLog(home = homedir()): string | undefined {
  const logsDir = join(home, ".mode", "v2", "logs");
  if (!existsSync(logsDir)) return undefined;
  const candidates = readdirSync(logsDir)
    .filter((name) => /^host-\d{4}-\d{2}-\d{2}\.log$/u.test(name))
    .sort()
    .reverse();
  return candidates.length > 0 ? join(logsDir, candidates[0]) : undefined;
}

function formatSample(marks: StartupMark[]): string {
  const lines: string[] = [];
  let previous = 0;
  for (const mark of marks) {
    lines.push(
      `  ${mark.phase.padEnd(12)} ${String(mark.elapsedMs).padStart(6)} ms` +
        `  (+${mark.elapsedMs - previous} ms)`,
    );
    previous = mark.elapsedMs;
  }
  return lines.join("\n");
}

function main(argv: string[]): number {
  const logIndex = argv.indexOf("--log");
  const logPath = logIndex >= 0 ? argv[logIndex + 1] : findLatestHostLog();
  if (!logPath) {
    console.error("[host-startup] 找不到 host-YYYY-MM-DD.log；先启动一次应用再跑。");
    return 1;
  }
  if (!existsSync(logPath)) {
    console.error(`[host-startup] 日志不存在：${logPath}`);
    return 1;
  }
  const marks = parseStartupMarks(readFileSync(logPath, "utf8"));
  if (marks.length === 0) {
    console.error(
      `[host-startup] ${logPath} 没有 startup_mark：该日志来自不含计时标记的构建；` +
        "重启一次应用后再跑。",
    );
    return 1;
  }
  console.log(`[host-startup] ${logPath}`);
  console.log(formatSample(marks));
  const ready = marks.find((mark) => mark.phase === "ready");
  const total = (ready ?? marks[marks.length - 1]).elapsedMs;
  console.log(
    `[host-startup] 合计 ${total} ms（起点为 Host 模块初始化；权威 T_ready 见冷启动脚本）`,
  );
  return 0;
}

const invokedDirectly =
  process.argv[1] !== undefined && pathToFileURL(process.argv[1]).href === import.meta.url;
if (invokedDirectly) {
  process.exitCode = main(process.argv.slice(2));
}
