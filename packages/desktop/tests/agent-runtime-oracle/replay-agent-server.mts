// C1 Task 10：Rust `agent-server` 的三层回放 harness（回放编排 + CLI 入口）。
//
// 用**同一份**投影/比较器（`normalize.mts` / `compare.mts`，spec §9.1 的单一比较器纪律）
// 把 oracle 行为夹具回放到 Rust `app-server --stdio` 上：
//
//   1. 起隔离数据根（mkdtemp，前缀与 `observeCase` 逐字相同——见下方「长度敏感性」）；
//   2. 预建**已迁移**会话库（C1 存储门是 fail-closed 的 schema 门，不建库；
//      DDL 从 `scripts/sqlite-schema-baseline.json` 生成，与 Rust 测试同一口径）；
//   3. 以 `MODE_AGENT_SERVER_SCRIPTED_PROVIDER=<夹具路径>` 拉起 Rust 二进制
//      （测试专用开关，生产路径不存在；见 `crates/agent-server/src/scripted.rs`）；
//   4. `session/create` + `session/send`（+ followUpTurns）驱动夹具剧本，应答反向请求
//      （偏好 → spec 默认；权限 → allow，与 `driver.mts` 的默认口径一致）；
//   5. 三层投影 → **C1 登记缺省过滤**（只丢 `session.updated` /
//      `streamRecovery.updated` / `session.titleUpdated` 三个已登记类型，两侧同删）
//      → `compareAll` 精确比较。
//
// 过滤是**过滤器**不是第二张规则表：白名单规则全部仍在 `normalize.mts`，
// 其余任何差异（未登记的）原样报出——那是本次回放要发现的东西。
// 驱动半边（spawn / 帧循环 / 反向请求应答）在 `agent-server-session.mts`，
// 二进制定位与基线库在 `agent-server-env.mts`（拆文件是为了守住 oxlint 的 max-lines）。
//
// **长度敏感性**：夹具 `expect` 里的 `inputByteLength` / `originalBytes` 等计数字段
// 随工作目录**绝对路径长度**变化，而三层白名单刻意不折叠它们（夹具要求录制与回放
// 在「同一隔离条件」下复现，spec §6.4）。本 harness 因此沿用 `observeCase` 的
// mkdtemp 前缀（`mode-oracle-fixture-` / `mode-oracle-fixture-ws-`），保证本机上
// 路径长度与录制运行一致；换机器需重录夹具（与 oracle 套件同一约束）。
//
// 单跑入口：`pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/replay-agent-server.mts [--case <name>]`
// 套件入口：`agent-server-replay.test.mts`（`pnpm test:agent-runtime-oracle` 自动纳入）。
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { compareAll, type Diff } from "./compare.mjs";
import { FIXTURE_CASES, generatedContent, loadFixtures, type OracleFixture } from "./fixtures.mjs";
import { seedWorkspace } from "./driver.mjs";
import {
  pickSessionEvents,
  projectEventStream,
  projectToolCalls,
  takeFileSnapshot,
  type Observed,
} from "./normalize.mjs";
import { startAgentServer, type AgentServerEvent } from "./agent-server-session.mjs";

/**
 * C1 登记缺省过滤（`agent-core-parity.md` §10 / `tauri-agent-runtime.md` §C1 差异登记）：
 * C1 不产出这三类事件，回放时**两侧同删**。除此之外不再有任何过滤。
 */
export const C1_ABSENT_EVENT_TYPES: readonly string[] = [
  "session.updated",
  "streamRecovery.updated",
  "session.titleUpdated",
];

const TERMINAL_EVENT_TYPES = new Set(["turn.completed", "turn.failed"]);
const TURN_TIMEOUT_MS = 90_000;

export type ReplayResult = {
  case: string;
  diffs: Diff[];
  /** 事件条数（过滤后；诊断用）。 */
  actualEventCount: number;
  expectedEventCount: number;
  /** Rust 侧 stderr（失败诊断用）。 */
  stderr: string;
};

/** 与 `observeCase` 相同的 mkdtemp 前缀（长度敏感性，见文件头）。 */
const DATA_ROOT_PREFIX = "mode-oracle-fixture-";
const WORKSPACE_PREFIX = "mode-oracle-fixture-ws-";

/**
 * 判断一个 `fixtures/` 根下的 JSON 是否是三层行为夹具。
 *
 * 目录里还住着**非三层**夹具（如 `session-stream.json`：M2 Task 5D 的投影快照，
 * `{case, direction, frame, note, cases}` 形状）——它们不是本轮回放的对象；
 * oracle 套件把它们当作三层夹具处理本身就是基线红的一部分（Task 10 不修）。
 * 本 harness 显式识别并跳过，报红与跳过都留痕，不静默。
 */
export function isThreeLayerFixture(value: unknown): value is OracleFixture {
  const candidate = value as Partial<OracleFixture>;
  return (
    typeof candidate?.case === "string" &&
    typeof candidate?.prompt === "string" &&
    Array.isArray(candidate?.script) &&
    typeof candidate?.expect === "object" &&
    candidate.expect !== null &&
    Array.isArray((candidate.expect as OracleFixture["expect"]).toolCalls)
  );
}

/** 落种子文件（`seed` / `seedBytesHex` / `generatedSeed` 三类，与 observeCase 同规则）。 */
async function seedFixtureWorkspace(
  cwd: string,
  fixture: Pick<OracleFixture, "seed" | "seedBytesHex" | "generatedSeed">,
): Promise<void> {
  await seedWorkspace(cwd, fixture.seed);
  for (const [relativePath, hex] of Object.entries(fixture.seedBytesHex ?? {})) {
    const target = join(cwd, relativePath);
    await mkdir(join(target, ".."), { recursive: true });
    await writeFile(target, Buffer.from(hex, "hex"));
  }
  for (const [relativePath, spec] of Object.entries(fixture.generatedSeed ?? {})) {
    await seedWorkspace(cwd, { [relativePath]: generatedContent(spec) });
  }
}

/**
 * 回放一条夹具：跑 Rust 会话 → 三层投影 → 两侧同删登记缺省事件 → `compareAll`。
 * 返回差异列表（空数组 = 三层一致）；所有失败都以异常抛出，不静默降级。
 */
export async function replayFixture(fixture: OracleFixture): Promise<ReplayResult> {
  const dataRoot = await mkdtemp(join(tmpdir(), DATA_ROOT_PREFIX));
  const cwd = await mkdtemp(join(tmpdir(), WORKSPACE_PREFIX));
  try {
    await seedFixtureWorkspace(cwd, fixture);
    const fixturePath = join(FIXTURE_DIR_ORACLE, `${fixture.case}.json`);
    const session = await startAgentServer({ dataRoot, cwd, fixturePath });
    let terminal: AgentServerEvent | undefined;
    try {
      const prompts = [fixture.prompt, ...(fixture.followUpTurns ?? []).map((turn) => turn.prompt)];
      for (const prompt of prompts) {
        await session.send(prompt);
        // 等本 turn 的新终态（frames 是追加数组，基线取 send 受理时的计数）。
        const baseline = countTerminals(session.frames);
        const deadline = Date.now() + TURN_TIMEOUT_MS;
        while (countTerminals(session.frames) <= baseline) {
          if (Date.now() > deadline) {
            throw new Error(
              `replay：会话在 ${TURN_TIMEOUT_MS}ms 内没有终态事件（${fixture.case}）。` +
                `stderr=${session.stderr}`,
            );
          }
          await delay(25);
        }
        terminal = session.frames.filter((event) => TERMINAL_EVENT_TYPES.has(event.type)).at(-1);
        if (terminal?.type === "turn.failed") {
          throw new Error(
            `replay：turn.failed（${fixture.case}）：${JSON.stringify(terminal.payload)}\nstderr=${session.stderr}`,
          );
        }
      }
      // 与 observeCase 相同的沉降窗：终态后的收尾帧（状态推送等）到达完毕。
      await delay(500);
      if (session.invalidLines.length > 0) {
        throw new Error(
          `replay：agent-server stdout 有解析失败行（${fixture.case}）：${session.invalidLines[0]}`,
        );
      }
      const context = { dataRoot, cwd, tmpDir: tmpdir() };
      const sessionFrames = pickSessionEvents(session.frames).filter(
        (event) => !C1_ABSENT_EVENT_TYPES.includes(event.type),
      );
      // 期望侧只能在**已规范化**的夹具事件上做同删（原始帧没有留存），因此无法
      // 重折无序组。夹具里的并行批次（≥2 调用）会让「实际侧先滤后折、期望侧先折后滤」
      // 的完成事件顺序不可比——显式拒绝而不是静默给出假差异（当前全部夹具都是
      // 单调用批次；引入并行批次夹具时需要把期望侧改为重录原始帧）。
      const hasParallelBatch = fixture.script.some(
        (step) => "toolCalls" in step && step.toolCalls.length > 1,
      );
      if (hasParallelBatch) {
        throw new Error(
          `replay：夹具 ${fixture.case} 含并行工具批次，期望侧已规范化事件无法做登记缺省同删（见 harness 注释）`,
        );
      }
      const expectedFrames = pickSessionEvents(
        (fixture.expect.events as AgentServerEvent[]).map((event) => ({
          type: event.type,
          payload: event.payload,
        })),
      ).filter((event) => !C1_ABSENT_EVENT_TYPES.includes(event.type));
      const observed: Observed = {
        toolCalls: projectToolCalls(sessionFrames, context),
        files: await takeFileSnapshot(cwd, context),
        events: projectEventStream(sessionFrames, context),
      };
      const expected: Observed = {
        toolCalls: fixture.expect.toolCalls,
        files: fixture.expect.files,
        // 期望侧的 events 在夹具里已是规范化形状（录制时投影过）；
        // 这里只做「同删过滤」，不再二次规范化（比较器要求两侧都是规范化观测）。
        events: expectedFrames,
      };
      return {
        case: fixture.case,
        diffs: compareAll(observed, expected),
        actualEventCount: observed.events.length,
        expectedEventCount: expected.events.length,
        stderr: session.stderr,
      };
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

const FIXTURE_DIR_ORACLE = fileURLToPath(new URL("./fixtures/", import.meta.url));

function countTerminals(frames: readonly AgentServerEvent[]): number {
  return frames.filter((event) => TERMINAL_EVENT_TYPES.has(event.type)).length;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolvePromise) => {
    setTimeout(resolvePromise, ms);
  });
}

// ---------------------------------------------------------------------------
// CLI 入口：单跑 / 全量（不进 node:test，便于人工排查）
// ---------------------------------------------------------------------------

const invoked = process.argv[1]
  ? fileURLToPath(pathToFileURL(process.argv[1]).href)
  : undefined;
const isMain = invoked !== undefined && invoked.toLowerCase() === fileURLToPath(import.meta.url).toLowerCase();
if (isMain) {
  const caseIndex = process.argv.indexOf("--case");
  const only = caseIndex >= 0 ? process.argv[caseIndex + 1] : undefined;
  const all = (await loadFixtures()) as unknown[];
  const skipped = all
    .filter((fixture) => !isThreeLayerFixture(fixture))
    .map((fixture) => (fixture as { case?: string }).case ?? "(unnamed)");
  const fixtures = all.filter(isThreeLayerFixture).filter(
    (fixture) => only === undefined || fixture.case === only,
  );
  for (const name of skipped) {
    process.stdout.write(`SKIP ${name}（非三层行为夹具，不属本轮回放）\n`);
  }
  if (fixtures.length === 0) {
    process.stderr.write(
      `没有匹配的夹具：${only ?? "(全部)"}。可选：${FIXTURE_CASES.map((entry) => entry.case).join(", ")}\n`,
    );
    process.exitCode = 1;
  } else {
    let green = 0;
    for (const fixture of fixtures) {
      const result = await replayFixture(fixture);
      if (result.diffs.length === 0) {
        green += 1;
        process.stdout.write(`GREEN ${fixture.case}\n`);
      } else {
        process.stdout.write(
          `RED ${fixture.case}（${result.diffs.length} 处差异）\n` +
            result.diffs
              .slice(0, 12)
              .map((diff) => `  - [${diff.layer}] ${diff.path}: ${diff.message}`)
              .join("\n") +
            "\n",
        );
      }
    }
    process.stdout.write(`\n${green}/${fixtures.length} 条夹具三层一致。\n`);
    if (green !== fixtures.length) process.exitCode = 1;
  }
}
