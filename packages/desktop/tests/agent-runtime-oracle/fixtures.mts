// M2 Task 0 Step 5：行为夹具的读取、录制与回放（spec §3.1）。
//
// 纪律：`prompt` / `seed` / `script` 是**输入**（人写）；`expect` 是**期望**（从 oracle 录，人不得手改）。
// 录制入口：`pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/record.mts`。
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { startOracleRuntime, seedWorkspace, type OracleEvent } from "./driver.mjs";
import type { MockStep } from "./mock-provider.mjs";
import {
  pickSessionEvents,
  projectEventStream,
  projectToolCalls,
  takeFileSnapshot,
  type FileSnapshot,
  type NormalizedEvent,
  type ToolCall,
} from "./normalize.mjs";

export type FixtureCase = {
  case: string;
  prompt: string;
  /** 运行前落进工作目录的种子文件（输入的一部分）。 */
  seed: Record<string, string>;
  /** mock provider 剧本；`${cwd}` 在运行时替换成实际工作目录。 */
  script: MockStep[];
};

export type Observed = {
  toolCalls: ToolCall[];
  files: FileSnapshot;
  events: NormalizedEvent[];
};

export type OracleFixture = FixtureCase & { expect: Observed };

export const FIXTURE_DIR = fileURLToPath(new URL("./fixtures/", import.meta.url));

const TERMINAL_EVENT_TYPES = new Set(["turn.completed", "turn.failed"]);

/** 三条夹具覆盖：只读一次成功 / 读不存在的文件（错误路径）/ 同一次 turn 里连续两次工具调用。 */
export const FIXTURE_CASES: readonly FixtureCase[] = [
  {
    case: "read-single",
    prompt: "用 Read 工具读一下 hello.txt，然后回一句话说明它写了什么。",
    seed: { "hello.txt": "hello\n" },
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/hello.txt" } }] },
      { text: "hello.txt 里写的是 hello。" },
    ],
  },
  {
    case: "read-missing",
    prompt: "用 Read 工具读一下 missing.txt；如果读不到，就回一句话说明失败原因。",
    seed: { "hello.txt": "hello\n" },
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/missing.txt" } }] },
      { text: "missing.txt 不存在，读取失败。" },
    ],
  },
  {
    case: "read-twice-one-turn",
    prompt: "在同一次回答里先读 a.txt、再读 b.txt，最后各回一句它们的内容。",
    seed: { "a.txt": "alpha\n", "b.txt": "beta\n" },
    // 两次调用分属两个批次（第二次调用由第二个模型请求发出），不是同一个 assistant 消息里的
    // 并行批次。理由见 spec §6「并行批次」：同一批次内的完成顺序是竞态，写进夹具会让用例随机红；
    // 「同一次 turn 里连续两次工具调用」这条要求由「两个批次」满足。
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/a.txt" } }] },
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/b.txt" } }] },
      { text: "a.txt 是 alpha，b.txt 是 beta。" },
    ],
  },
];

export async function loadFixtures(): Promise<OracleFixture[]> {
  const entries = await readdir(FIXTURE_DIR);
  const fixtures: OracleFixture[] = [];
  for (const entry of entries.filter((name) => name.endsWith(".json")).sort()) {
    const text = await readFile(join(FIXTURE_DIR, entry), "utf8");
    fixtures.push(JSON.parse(text) as OracleFixture);
  }
  return fixtures;
}

export async function writeFixture(fixture: OracleFixture): Promise<void> {
  await mkdir(FIXTURE_DIR, { recursive: true });
  await writeFile(
    join(FIXTURE_DIR, `${fixture.case}.json`),
    `${JSON.stringify(fixture, null, 2)}\n`,
    "utf8",
  );
}

/** 跑一次 oracle 并投影出三层观测。用独立的临时根与工作目录，跑完即清理。 */
export async function observeCase(fixtureCase: FixtureCase): Promise<Observed> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-oracle-fixture-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-oracle-fixture-ws-"));
  try {
    await seedWorkspace(cwd, fixtureCase.seed);
    const session = await startOracleRuntime({
      dataRoot,
      cwd,
      script: fixtureCase.script,
    });
    let terminal: OracleEvent | undefined;
    try {
      await session.send(fixtureCase.prompt);
      const deadline = Date.now() + 90_000;
      for await (const event of session.events) {
        if (TERMINAL_EVENT_TYPES.has(event.type)) {
          terminal = event;
          break;
        }
        if (Date.now() > deadline) {
          throw new Error(`oracle 会话在 90s 内没有终态事件（${fixtureCase.case}）`);
        }
      }
      // 终态之后可能还有收尾帧（标题、状态推送）；给一小段沉降时间再取快照。
      await new Promise((resolve) => setTimeout(resolve, 500));
      const context = { dataRoot, cwd, tmpDir: tmpdir() };
      const sessionFrames = pickSessionEvents(session.frames);
      const observed: Observed = {
        toolCalls: projectToolCalls(sessionFrames, context),
        files: await takeFileSnapshot(cwd, context),
        events: projectEventStream(sessionFrames, context),
      };
      if (terminal === undefined) {
        throw new Error(`oracle 会话没有终态事件（${fixtureCase.case}）`);
      }
      if (terminal.type === "turn.failed") {
        throw new Error(
          `oracle 会话以 turn.failed 结束（${fixtureCase.case}）：${JSON.stringify(terminal.payload)}`,
        );
      }
      return observed;
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}
