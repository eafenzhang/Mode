// M2 Task 2 探针：`Read` 的**会话内历史**（重复读去重）——诊断用，不进对拍套件。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-read-history.mts [dedup|dedup2|dedup3]
//
// ## 结论（2026-10-16 实测；原始输出见运行日志）
//
// 同一会话里**用同样的入参**第二次读同一个文件，拿到的不是正文而是：
//   `Wasted call — file unchanged since your last Read. Refer to that earlier tool_result instead.`
// 它是一条**成功**结果（`success: true`，`originalBytes: 95`——95 字节 / JS 里 93 个字符，
// 差在破折号是多字节）。
//
// | 问题 | 观察 | 出处 |
// | --- | --- | --- |
// | 去重键是什么 | **解析后的路径 + range**：换 `offset` 就回正文；相对路径与绝对路径是**同一个键** | `dedup` 3 / 6 |
// | 别的文件读一遍会不会重置 | 不会（换了 `other.txt` 再读原文件仍是填充语） | `dedup` 5 |
// | 错误会被去重吗 | **不会**：读不存在的文件两次都是错误 | `dedup` 7-8 |
// | 跨 turn 还算吗 | **算**（状态是会话级的）：第二个 turn 里再读仍是填充语 | `dedup` 9-10 |
// | 文件变了呢 | 给正文（变更会打破去重） | `dedup` 11 |
// | 判「变了」看大小吗 | **不是只看大小**：同大小改写也给正文 | `dedup2` 3 |
// | 看 mtime 还是内容 | **mtime**：只碰 mtime（内容一字未改）→ 正文；改内容但把 mtime 恢复 → 仍填充语 | `dedup3` 3 / 6 |
//
// 所以 Rust 侧的会话状态按 **(解析后的路径, offset, limit) → 该次读时的 mtime** 记，命中且 mtime
// 相同才回填充语；错误不进这张表。实现见 `agent_tools::ReadHistory`（状态挂在会话上，
// 工具本身保持纯函数）。
//
// **踩过的坑**：同一条对话里「接着 send 一次」**不等于**新 turn——运行时的 turn 只在模型给出收尾
// 文本时才结束。所以剧本在两组读之间插了 `{ text: … }`，否则第二步的读其实还在第一个 turn 里
// （第一版探针就这么错了，「跨 turn」一度是假的）。
import { statSync, utimesSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { seedWorkspace, startOracleRuntime, type OracleEvent } from "./driver.mjs";
import { describeOutcome, drain, rawToolOutcome, type MockStep } from "./probe-support.mjs";

/** 一次「读一个文件」的剧本步。 */
function readStep(filePath: string, extra: Record<string, unknown> = {}): MockStep {
  return { toolCalls: [{ name: "Read", input: { file_path: filePath, ...extra } }] };
}

/** 打印第 `index` 次调用的原始结果。 */
function report(frames: readonly OracleEvent[], index: number): void {
  const observed = rawToolOutcome(frames, index);
  process.stdout.write(`\n== step ${index + 1} ==\n`);
  if (!observed) {
    process.stdout.write("(无结果)\n");
    return;
  }
  process.stdout.write(`ok: ${observed.ok}\n${describeOutcome(observed)}\n`);
}

/**
 * 去重的四条边界（都在一个会话里，跨 turn 也问）。
 *
 * 前 8 次读是第一个 turn，中间夹一条收尾文本，后 3 次读是第二个 turn；第 11 次读之前用
 * mock 请求钩子改写文件——用来观察「文件变了就给正文」。
 */
const DEDUP_STEPS: { filePath: string; extra?: Record<string, unknown> }[] = [
  { filePath: "${cwd}/three.txt" },
  { filePath: "${cwd}/three.txt" },
  { filePath: "${cwd}/three.txt", extra: { offset: 2 } },
  { filePath: "${cwd}/other.txt" },
  { filePath: "${cwd}/three.txt" },
  { filePath: "three.txt" },
  { filePath: "${cwd}/absent.txt" },
  { filePath: "${cwd}/absent.txt" },
  { filePath: "${cwd}/three.txt" },
  { filePath: "${cwd}/three.txt", extra: { offset: 2 } },
  { filePath: "${cwd}/three.txt" },
];
const DEDUP_TURN1_STEPS = 8;
const DEDUP_MUTATE_AT_REQUEST = 12;

async function runDedup(): Promise<number> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  try {
    await seedWorkspace(cwd, { "three.txt": "1\n2\n3\n", "other.txt": "other\n" });
    const script: MockStep[] = [
      ...DEDUP_STEPS.slice(0, DEDUP_TURN1_STEPS).map((step) => readStep(step.filePath, step.extra)),
      { text: "第一个 turn 结束" },
      ...DEDUP_STEPS.slice(DEDUP_TURN1_STEPS).map((step) => readStep(step.filePath, step.extra)),
      { text: "dedup 探针结束" },
    ];
    let requestIndex = 0;
    const session = await startOracleRuntime({
      dataRoot,
      cwd,
      script,
      onMockRequestBody: () => {
        requestIndex += 1;
        if (requestIndex === DEDUP_MUTATE_AT_REQUEST) {
          writeFileSync(join(cwd, "three.txt"), "1\n2\n3\nCHANGED\n");
          process.stdout.write("\n[fyi] 第 11 步之前改写 three.txt\n");
        }
      },
    });
    try {
      process.stdout.write(`\n########## dedup（cwd=${cwd}）##########\n`);
      await session.send("先读几遍同一个文件");
      await drain(session.events);
      process.stdout.write("\n=== turn 2（同一会话，新 turn）===\n");
      await session.send("再读一遍同一个文件");
      await drain(session.events);
      for (let index = 0; index < DEDUP_STEPS.length; index += 1) report(session.frames, index);
      return DEDUP_STEPS.length;
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

/** 判「变了」看**大小**吗：把 `four.txt` 换成**同大小**的另一段内容（第 3 步之前注入）。 */
const DEDUP2_ORIGINAL = "1\n2\n3\n";
const DEDUP2_CHANGED = "9\n8\n7\n";

async function runDedup2(): Promise<number> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  try {
    const target = join(cwd, "four.txt");
    await seedWorkspace(cwd, { "four.txt": DEDUP2_ORIGINAL });
    const script: MockStep[] = Array.from({ length: 5 }, () => readStep("${cwd}/four.txt"));
    script.push({ text: "dedup2 探针结束" });
    let requestIndex = 0;
    const session = await startOracleRuntime({
      dataRoot,
      cwd,
      script,
      onMockRequestBody: () => {
        requestIndex += 1;
        if (requestIndex === 3) {
          writeFileSync(target, DEDUP2_CHANGED);
          process.stdout.write("\n[fyi] 第 3 步之前：同大小改写（内容变了）\n");
        }
        if (requestIndex === 5) {
          writeFileSync(target, DEDUP2_ORIGINAL);
          process.stdout.write("\n[fyi] 第 5 步之前：改回原内容\n");
        }
      },
    });
    try {
      process.stdout.write(`\n########## dedup2（cwd=${cwd}）##########\n`);
      await session.send("看几遍同一个文件");
      await drain(session.events);
      for (let index = 0; index < 5; index += 1) report(session.frames, index);
      return 5;
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

/**
 * **mtime 还是内容**：两条互补的注入。
 *
 * | 步骤 | 注入 | mtime 假设 | 内容假设 |
 * | --- | --- | --- | --- |
 * | 3 | 只碰 `x.txt` 的 mtime（内容一字未改） | 正文 | **填充语** |
 * | 6 | 改 `y.txt` 的内容**并把 mtime 恢复成原值** | **填充语** | 正文 |
 */
const DEDUP3_MTIME_BUMP_MS = 5_000;

async function runDedup3(): Promise<number> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  try {
    await seedWorkspace(cwd, { "x.txt": "1\n2\n3\n", "y.txt": "a\nb\n" });
    const xPath = join(cwd, "x.txt");
    const yPath = join(cwd, "y.txt");
    const xTimes = statSync(xPath);
    const yTimes = statSync(yPath);
    const script: MockStep[] = [
      readStep(xPath),
      readStep(xPath),
      readStep(xPath),
      readStep(yPath),
      readStep(yPath),
      readStep(yPath),
      { text: "dedup3 探针结束" },
    ];
    let requestIndex = 0;
    const session = await startOracleRuntime({
      dataRoot,
      cwd,
      script,
      onMockRequestBody: () => {
        requestIndex += 1;
        if (requestIndex === 3) {
          const bumped = new Date(xTimes.mtimeMs + DEDUP3_MTIME_BUMP_MS);
          utimesSync(xPath, bumped, bumped);
          process.stdout.write("\n[fyi] 第 3 步之前：只把 x.txt 的 mtime 往后拨 5 秒\n");
        }
        if (requestIndex === 6) {
          writeFileSync(yPath, "A\nB\n");
          utimesSync(yPath, yTimes.atime, yTimes.mtime);
          process.stdout.write("\n[fyi] 第 6 步之前：改 y.txt 的内容并把 mtime 恢复成原值\n");
        }
      },
    });
    try {
      process.stdout.write(`\n########## dedup3（cwd=${cwd}）##########\n`);
      await session.send("看几遍同一个文件");
      await drain(session.events);
      for (let index = 0; index < 6; index += 1) report(session.frames, index);
      return 6;
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

const RUNNERS: Record<string, () => Promise<number>> = {
  dedup: runDedup,
  dedup2: runDedup2,
  dedup3: runDedup3,
};

const requested = process.argv.slice(2);
const names = requested.length > 0 ? requested : Object.keys(RUNNERS);
for (const name of names) {
  const run = RUNNERS[name];
  if (!run) {
    process.stdout.write(`未知组名：${name}\n`);
    continue;
  }
  const probes = await run();
  process.stdout.write(`\n##### ${name}: ${probes} 个探针 #####\n`);
}
