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
  /**
   * **原始字节**种子（十六进制文本）：`seed` 只能表达 utf8 文本，而
   * `Read` 的「非文本编码」分支必须用真的含 NUL 的字节才能触发（Task 2 观察：
   * `.txt` 扩展名 + NUL 字节 → `FileSystemPortError` / `code=unsupported`）。
   */
  seedBytesHex?: Record<string, string>;
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
  // ---- Task 2 追加：`Read` 的编排名义与边界（每个 Read 各占一个 assistant 消息 = 一个批次，
  //      避免把并行批次的完成顺序写进夹具；见 spec §6.5）。
  {
    case: "read-offset-edges",
    prompt: "依次读这几个文件，把每次读到的原文都贴出来。",
    seed: {
      "o1.txt": "1\n2\n3\n",
      "o2.txt": "1\n2\n3\n",
      "o3.txt": "1\n2\n3\n",
      "empty.txt": "",
      "crlf.txt": "x\r\ny\r\n",
      "rel.txt": "rel\n",
    },
    script: [
      // 行号从 offset 起算，编号 = offset + 返回序号（观察：offset 1 → `1\t1\n2\t2\n3\t3\n4\t`）。
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/o1.txt", offset: 1 } }] },
      // limit 是**行数**不是结束行号。
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/o2.txt", offset: 2, limit: 1 } }] },
      // offset 越过最后一行 → success + `<system-reminder>Warning: …shorter than the provided offset (4). The file has 4 lines.…`（成功而非错误）。
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/o3.txt", offset: 4 } }] },
      // 空文件按 1 行算（末尾空段），因此默认 offset 1 就落在警告分支。
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/empty.txt" } }] },
      // CRLF：行尾 `\r` 被去掉，仍按 3 段编号。
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/crlf.txt" } }] },
      // 相对路径按会话工作目录解析（工具描述说必须绝对路径，实测不然）。
      { toolCalls: [{ name: "Read", input: { file_path: "rel.txt" } }] },
      { text: "都读完了。" },
    ],
  },
  // ---- Task 2 追加：`Read` 的错误分类（每个分支一个调用，错误形状逐字进夹具）。
  {
    case: "read-error-paths",
    prompt: "依次做这些读取，把每次的结果原样说一遍。",
    seed: { "hello.txt": "hello\n", "dir/inner.txt": "inner\n", "blob.bin": "not really binary\n" },
    // 文件名**不能**叫 `nul.txt`：Windows 上 `nul.<任意扩展名>` 是老式 NUL 设备名，
    // 写进去的字节会被丢弃、读出来是空——那会录到一个假观察。
    seedBytesHex: { "nulbytes.txt": "6100620a" },
    script: [
      // 不存在 → tool_execution_failed / read_file_not_found，message 里带会话工作目录。
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/absent.txt" } }] },
      // 目录 → FileSystemPortError / is_directory（注意：与上一条的 error.type 不同）。
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/dir" } }] },
      // 扩展名 `.bin` → 读前预检拒绝（与文件内容无关：这个 .bin 里其实是普通文本）。
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/blob.bin" } }] },
      // `.txt` + NUL 字节 → 走**另一个**错误：读文件端口报「不支持的文本编码」。
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/nulbytes.txt" } }] },
      // limit 0 → zod 精化失败（error.type 是 ZodError，message 是序列化后的 issue 数组）。
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/hello.txt", limit: 0 } }] },
      // offset 不是整数 → 走 inputSchema 校验拒绝（与上一条又不同）。
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/hello.txt", offset: "2" } }] },
      // 空 file_path → 路径非空校验。
      { toolCalls: [{ name: "Read", input: { file_path: "" } }] },
      { text: "七个分支都试过了。" },
    ],
  },
  // ---- Task 2 追加：**只读切片里不存在的两个工具**（`Glob` / `Grep`）。
  //      它们仍由脚本化调用发出，因为要录的正是「运行时对它们的真实答复」。
  {
    case: "read-absent-search-tools",
    prompt: "试一下这两个工具，把结果原样说出来。",
    seed: { "one.txt": "alpha\n" },
    script: [
      { toolCalls: [{ name: "Glob", input: { pattern: "*.txt" } }] },
      { toolCalls: [{ name: "Grep", input: { pattern: "alpha", path: "${cwd}" } }] },
      { text: "两个都不是可用工具。" },
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
    for (const [relativePath, hex] of Object.entries(fixtureCase.seedBytesHex ?? {})) {
      const target = join(cwd, relativePath);
      await mkdir(join(target, ".."), { recursive: true });
      await writeFile(target, Buffer.from(hex, "hex"));
    }
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
