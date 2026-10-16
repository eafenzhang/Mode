/**
 * 「跑**安装目录里的**产品壳」这件事的探针（M3 Task 5）。
 *
 * 为什么单独一层：`verify-installed.ts`（装完之后的判定）与 `measure-cold-start.ts`
 * （按 spec §13.3 的口径量冷启动）要的是同一套动作——起产品壳、抓 stderr、认两个标记、
 * 收尾不留孤儿——只是判定不同。把动作写在两处，必然漂移成「两次测的不是同一件事」。
 *
 * ## 三个刻意的取舍
 *
 * 1. **标记取自既有的诊断面原文**，不加探针：`[mode-host-sidecar] ready …`（Host 打的，
 *    经壳的 stderr 转发）与 `[mode-tauri] platform_surface …`（注入层上报、壳打印）。
 *    加探针就是改被测对象（spec §13.3）。
 * 2. **stderr 一律走管道**：发布构建带 `windows_subsystem = "windows"`，没有控制台——
 *    但父进程把 `STDERR_HANDLE` 继承给它之后，`eprintln!` 照样写得出来。
 *    这正是 §20 第 1 条那条缺口的可观测半边（另一半是「用户从资源管理器启动时没有落点」，
 *    仍登记为未关闭）。
 * 3. **关窗口用 `taskkill`（不带 `/F`）**：它给顶层窗口发 `WM_CLOSE`，走的是产品的正常退出路径
 *    （`RunEvent::Exit` → 收 Host）。发不动再退到 `taskkill /T /F`，并**如实记下用了哪一种**——
 *    强杀与正常退出对「有没有留孤儿」是两件不同的事。
 */
import { spawn, spawnSync } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";

export interface ShellTrace {
  /** 全部 stderr/stdout 行，带到达时刻（毫秒，相对 `spawn`）。 */
  lines: { at: number; stream: "out" | "err"; text: string }[];
  /** 标记 → 到达时刻；没出现就是 `undefined`（不用 0 假装出现过）。 */
  marks: Record<string, number | undefined>;
  /** 收尾方式：`wm_close` = 走正常退出路径；`kill_tree` = 强杀。 */
  closedBy: "wm_close" | "kill_tree";
  /** 进程已退时的退出码（我们收尾之前就退的情况）。 */
  exitCode: number | null;
}

export interface RunShellOptions {
  exe: string;
  dataBaseDir: string;
  home: string;
  /** 每个标记出现即记时刻；字符串按 `includes` 匹配。 */
  marks: { key: string; needle: string }[];
  /** 等最后一个标记的上限。 */
  timeoutMs: number;
  /** 认到全部标记后再观察这么久（让后续输出也进 trace）。 */
  settleMs: number;
}

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/** 起产品壳、抓到全部标记、然后收尾；返回原始痕迹（判定留给调用方）。 */
export async function runProductShell(options: RunShellOptions): Promise<ShellTrace> {
  const started = Date.now();
  const child = spawn(options.exe, [], {
    env: {
      ...process.env,
      MODE_DATA_BASE_DIR: options.dataBaseDir,
      HOME: options.home,
      USERPROFILE: options.home,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const trace: ShellTrace = { lines: [], marks: {}, closedBy: "kill_tree", exitCode: null };
  const record = (stream: "out" | "err", chunk: Buffer): void => {
    const at = Date.now() - started;
    for (const text of chunk.toString().split(/\r?\n/u)) {
      if (text.length === 0) continue;
      trace.lines.push({ at, stream, text });
      for (const mark of options.marks) {
        if (trace.marks[mark.key] === undefined && text.includes(mark.needle)) {
          trace.marks[mark.key] = at;
        }
      }
    }
  };
  child.stdout.on("data", (chunk: Buffer) => record("out", chunk));
  child.stderr.on("data", (chunk: Buffer) => record("err", chunk));
  let exited = false;
  child.once("exit", (code) => {
    exited = true;
    trace.exitCode = code;
  });

  const deadline = started + options.timeoutMs;
  while (Date.now() < deadline && !exited) {
    if (options.marks.every((mark) => trace.marks[mark.key] !== undefined)) break;
    await sleep(25);
  }
  await sleep(options.settleMs);
  if (!exited) trace.closedBy = await closeShell(child.pid ?? 0);
  return trace;
}

/**
 * 关掉产品壳：先发 `WM_CLOSE`（正常退出路径），5 秒没退再强杀整棵进程树。
 *
 * 强杀那一档同时覆盖 sidecar：壳被强杀时 Host 靠 stdin EOF 自己退（DR-T1-3 的兜底），
 * 所以两种收尾都**不该**留下 `node.exe`——那正是调用方要断言的。
 */
export async function closeShell(pid: number): Promise<"wm_close" | "kill_tree"> {
  if (pid <= 0) return "kill_tree";
  spawnSync("taskkill", ["/PID", String(pid)], { stdio: "ignore" });
  if (await waitForProcessGone(pid, 5_000)) return "wm_close";
  spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
  await waitForProcessGone(pid, 10_000);
  return "kill_tree";
}

/** 轮询一个 pid 是否消失（`tasklist` 按 pid 过滤；查不到即「已退」）。 */
export async function waitForProcessGone(pid: number, timeoutMs: number): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (!isProcessAlive(pid)) return true;
    if (Date.now() >= deadline) return false;
    await sleep(250);
  }
}

/** 单个 pid 是否还在（`/NH` 去掉表头；输出的 `INFO:` 行表示没有匹配）。 */
function isProcessAlive(pid: number): boolean {
  const result = spawnSync("tasklist", ["/FI", `PID eq ${pid}`, "/NH"], { encoding: "utf8" });
  return /\d/u.test(String(result.stdout));
}

/**
 * 列出**可执行文件路径落在 `underDir` 里**的进程（按名字查，再按路径过滤）。
 *
 * 为什么不能只按名字：这台机器上本来就有别的 `node.exe`（仓库自己的构建、用户自己的），
 * 「有 `node.exe` 在跑」证明不了是我们留下的孤儿。同理也不能只按 pid——
 * 孤儿的特点就是换了个 pid 活着。
 */
export function listProcessesUnder(
  name: string,
  underDir: string,
): { pid: number; path: string }[] {
  const script =
    `Get-CimInstance Win32_Process -Filter "Name='${name}'" | ` +
    "Select-Object ProcessId,ExecutablePath | ConvertTo-Json -Compress";
  const result = spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], {
    encoding: "utf8",
  });
  const raw = String(result.stdout).trim();
  if (raw.length === 0) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  const rows = Array.isArray(parsed) ? parsed : [parsed];
  const prefix = underDir.toLowerCase();
  return rows
    .map((row) => row as { ProcessId?: unknown; ExecutablePath?: unknown })
    .filter(
      (row) =>
        typeof row.ExecutablePath === "string" &&
        row.ExecutablePath.toLowerCase().startsWith(prefix),
    )
    .map((row) => ({ pid: Number(row.ProcessId), path: String(row.ExecutablePath) }));
}

/** 一个目录的条目名集合（排序）。目录不存在返回 `null`——「不存在」与「空」是两件事。 */
export function entryNames(dir: string): string[] | null {
  if (!existsSync(dir)) return null;
  return readdirSync(dir).sort();
}
