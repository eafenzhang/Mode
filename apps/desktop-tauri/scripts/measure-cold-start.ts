/**
 * 冷启动实测（M3 Task 5 · Step 4，口径见 `docs/specs/tauri-renderer-host.md` §13.3）。
 *
 * **口径先于数字**：两个段（`T_ready` / `T_surface`）、每样本一个新空数据基目录、
 * N = 7、统计量取**中位数**并与最小/最大值一起报、第 1 个样本单列（安装后首次，
 * 文件缓存也冷）——这些在测之前就写进了 spec，本脚本只是执行它。别在这里改口径，
 * 改了就跟 spec 分叉了。
 *
 * 两个标记都是**既有诊断面的原文**（Host 的 `ready` 行、壳的 `platform_surface` 行），
 * 不新增探针：加探针就是改被测对象。
 *
 * ```bash
 * node --import tsx apps/desktop-tauri/scripts/measure-cold-start.ts --release <exe>
 * node --import tsx apps/desktop-tauri/scripts/measure-cold-start.ts --debug <exe>
 * ```
 */
import { existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";

import { listProcessesUnder, runProductShell, type ShellTrace } from "./shell-process-probe.ts";

/** 口径固定值（改这里就是改口径，必须同步 spec §13.3）。 */
const SAMPLES = 7;
const TIMEOUT_MS = 120_000;
const SETTLE_MS = 2_000;
const MARKS = [
  { key: "ready", needle: "[mode-host-sidecar] ready startupId=" },
  { key: "surface", needle: "[mode-tauri] platform_surface implemented=" },
];

interface Sample {
  index: number;
  readyMs: number | undefined;
  surfaceMs: number | undefined;
  closedBy: ShellTrace["closedBy"];
  failed: boolean;
}

function median(values: number[]): number {
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? Math.round(((sorted[middle - 1] as number) + (sorted[middle] as number)) / 2)
    : (sorted[middle] as number);
}

function describe(values: number[]): string {
  return `中位数 ${String(median(values))} ms，最小 ${String(Math.min(...values))} ms，最大 ${String(Math.max(...values))} ms（n=${String(values.length)}）`;
}

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  const labelIndex = argv.findIndex((arg) => arg === "--release" || arg === "--debug");
  if (labelIndex < 0) throw new Error("需要一个档：--release <exe> 或 --debug <exe>");
  const label = argv[labelIndex] === "--release" ? "release" : "debug";
  const exe = resolve(argv[labelIndex + 1] ?? "");
  if (!existsSync(exe)) throw new Error(`可执行文件不存在：${exe}`);

  const workDir = join(tmpdir(), `mode-m3t5-coldstart-${String(process.pid)}`);
  rmSync(workDir, { recursive: true, force: true });
  mkdirSync(workDir, { recursive: true });
  // 残留检查的基准目录：**随包 Node** 所在的那一层（`<install>/node/`）。
  // 调试档没有随包 Node（壳走 PATH 上的 `node`），所以那里按路径过滤检查不了——
  // 如实报 null，不假装检查过。
  const bundledNodeDir = existsSync(join(dirname(exe), "node")) ? join(dirname(exe), "node") : null;
  console.log(`[cold-start] 档=${label} exe=${exe} ${String(statSync(exe).size)} B`);
  console.log(
    `[cold-start] 样本间的残留检查：${
      bundledNodeDir === null
        ? "本档不适用（没有随包 Node，壳走 PATH）"
        : `按路径过滤 ${bundledNodeDir}`
    }`,
  );
  console.log(
    `[cold-start] 口径：每样本一个新空数据基目录 + HOME/USERPROFILE 双覆写；N=${String(SAMPLES)}；` +
      "统计量=中位数；第 1 个样本单列（首次，文件缓存也冷）",
  );

  const samples: Sample[] = [];
  for (let index = 1; index <= SAMPLES; index += 1) {
    const dataBase = join(workDir, `sample-${String(index)}`, "base");
    const home = join(workDir, `sample-${String(index)}`, "home");
    mkdirSync(dataBase, { recursive: true });
    mkdirSync(home, { recursive: true });
    const trace = await runProductShell({
      exe,
      dataBaseDir: dataBase,
      home,
      marks: MARKS,
      timeoutMs: TIMEOUT_MS,
      settleMs: SETTLE_MS,
    });
    const text = trace.lines.map((line) => line.text).join("\n");
    const sample: Sample = {
      index,
      readyMs: trace.marks.ready,
      surfaceMs: trace.marks.surface,
      closedBy: trace.closedBy,
      failed: text.includes("startup_failed") || trace.marks.ready === undefined,
    };
    samples.push(sample);
    console.log(
      `[cold-start] 样本 ${String(index)}: T_ready=${String(sample.readyMs)} ms ` +
        `T_surface=${String(sample.surfaceMs)} ms 收尾=${sample.closedBy} 失败=${String(sample.failed)}`,
    );
    // 样本之间确认无残留：上一个样本的 Host/pty 活着会影响下一个样本（口径要求）。
    // 只在**有随包 Node** 的档上做这件事——调试档走 PATH 上的 `node`，按路径过滤就找不到它，
    // 那种「检查永远通过」的假绿比不做更坏，所以如实报「本档不适用」。
    if (bundledNodeDir !== null) {
      const orphans = listProcessesUnder("node.exe", bundledNodeDir);
      if (orphans.length > 0) {
        throw new Error(
          `样本 ${String(index)} 后有残留 node.exe（整轮作废）：` +
            orphans.map((entry) => `${String(entry.pid)} ${entry.path}`).join(", "),
        );
      }
    }
  }

  const ok = samples.filter((sample) => !sample.failed);
  const ready = ok.map((sample) => sample.readyMs as number);
  const surface = ok.map((sample) => sample.surfaceMs as number);
  console.log("");
  console.log(`[cold-start] === 结论（档=${label}）===`);
  console.log(
    `[cold-start] 有效样本 ${String(ok.length)}/${String(samples.length)}（失败=没出现 ready 行或出现 startup_failed）`,
  );
  if (ok.length > 0) {
    const first = ok[0] as Sample;
    console.log(
      `[cold-start] 第 1 个样本（首次，文件缓存也冷）：T_ready=${String(first.readyMs)} ms T_surface=${String(first.surfaceMs)} ms`,
    );
    console.log(`[cold-start] T_ready   全部有效样本 ${describe(ready)}`);
    console.log(`[cold-start] T_surface 全部有效样本 ${describe(surface)}`);
    const rest = ok.slice(1);
    if (rest.length > 0) {
      console.log(
        `[cold-start] 去掉第 1 个之后的 T_ready   ${describe(rest.map((sample) => sample.readyMs as number))}`,
      );
      console.log(
        `[cold-start] 去掉第 1 个之后的 T_surface ${describe(rest.map((sample) => sample.surfaceMs as number))}`,
      );
    }
  }
  if (samples.length !== ok.length) process.exitCode = 1;
}

void main().catch((error: unknown) => {
  console.error(
    `[cold-start] 失败：${error instanceof Error ? (error.stack ?? error.message) : String(error)}`,
  );
  process.exitCode = 1;
});
