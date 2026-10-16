// M2 Task 2：只读工具（`Read` / `Glob` / `Grep`）的**行为探针**（诊断用，不进对拍套件）。
//
// 纪律（docs/specs/clean-room-policy.md §黑盒对拍）：只运行旧实现、观察它的输入输出与事件序列。
// 本文件不读旧实现的实现体，也不把观察「手写」进夹具——夹具由 `record.mts` 录（`fixtures.mts`
// 的 `FIXTURE_CASES` 是人写输入、`expect` 由 oracle 录制）。
//
// 与夹具的分工：
//   - 夹具（`fixtures/*.json`）钉住**契约**：每条用例的三层投影，回放用例逐字复现；
//   - 本探针只服务**发现**：批量跑一批边角入参、把原始结果逐字打出来（不规范化、不截断），
//     用来回答「截断上限是多少 / 行号怎么编号 / 无匹配是不是错误」这类问题。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-read-only.mts [组名…]
//   组名：tools | read | trunc | limit-edge | absence（不传 = 全跑）
//
// ## 本探针回答的问题与结论（2026-10-16 实测，原始输出见运行日志）
//
// 1. **运行时对外声明的工具清单只有 21 条**，其中只读切片里**只有 `Read` 注册**：
//    `Glob` / `Grep` **不在这 21 条里**，而且脚本化调用它们会得到
//    `{"type":"tool_not_found","message":"Tool not found: Glob","code":"TOOL_NOT_FOUND"}`——
//    即它们**在会话面根本没有注册**（不是「注册了但看不见」）。见 `absence` 组。
//    这与计划里「只读切片 = Read/Glob/Grep」的写法冲突（那个写法是从 `MODE_KNOWN_TOOL_NAMES`
//    这张**渲染用名字表**推出来的，不是从运行时注册表推出来的）。本组是这条结论的证据。
// 2. **清单不随 CLI 参数变化**：`--surface desktop`、`--mode build|plan`、`--disallowed-tools`
//    都不改变这 21 条（正对照：`--disallowed-tools Bash` 也照样声明 `Bash`）——说明 app-server
//    的会话面工具表不是由这些 prompt 期参数决定的。
// 3. `nativeSearchEnhancementsEnabled` 也不改变它。该偏好的语义由协议声明给出
//    （`packages/shared/src/protocol.ts` 的字段注释：**「新建或冷恢复 Session 是否为 Bash
//    注入 bfs/ugrep 增强」**）——它改的是 Bash 的注入，不是工具注册表。
// 4. **`Read` 的截断模型**（`trunc` 与 `limit-edge` 两组，逐点实测）：
//    - 估算口径 = `ceil(字节数 / 3)`：74999 B → 全文；75000 B → 全文；75001 B → 25001 tokens 超限。
//      （`floor` 与 75001 那一点矛盾，所以只能是上取整。）上限 **25000 tokens**，严格大于才触发。
//    - 上限作用在**被选中的范围**（`offset`/`limit` 选出的那些行，按源文本字节数算），不是整个文件：
//      2500×60 的文件 `offset:1, limit:2000` → 121999 B → 40667 tokens；`offset: 2` → 152438 B → 50813。
//    - **给了 `offset` 或 `limit` 任一**且超限 → 错误 `read_output_too_many_tokens`；
//      **两个都不给**且超限 → 成功，正文是「部分视图」（提醒头 + 前若干行）。提醒头文案已逐字
//      录到，但**截取多少行**的规则从一个样本反推不出来 → `read.rs` 标为 M2 未实现的面。
//    - **256KB 大小上限只在两个都不给的路径上生效**：304999 B 的文件加 `limit: 5000` 得到的是
//      token 错误（101667 tokens），不是大小错误。
//
// ## 踩过的坑（留给下一个人）
//
// Windows 上 `nul.<任意扩展名>`（以及 `con`/`aux`/`prn`/`com1`…）是**设备名**：写进去的字节被
// 丢弃、读出来是空。用 `nul.txt` 当「含 NUL 字节的文本文件」会录到一个假观察（本探针第一版
// 就踩了，夹具里已改名 `nulbytes.txt`）。
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { seedWorkspace, startOracleRuntime, type OracleEvent } from "./driver.mjs";
import type { MockStep } from "./mock-provider.mjs";

type Probe = {
  label: string;
  /** 覆盖组默认工具名（`absence` 组要同时探 Glob 与 Grep）。 */
  tool?: string;
  /** UTF-8 种子文件。 */
  seed?: Record<string, string>;
  /** **原始字节**的种子（二进制探测）：`seedWorkspace` 只收 utf8 文本。 */
  seedBytes?: Record<string, Buffer>;
  args: Record<string, unknown>;
};
type Group = { tool: string; name: string; probes: Probe[] };

const BIG_LINES = Array.from({ length: 2500 }, () => "z".repeat(60)).join("\n");
const ONE_LONG_LINE = "q".repeat(60_000);

/**
 * `Read` 的边角入参：行号编号与 `offset`/`limit` 边界、截断上限、二进制判定、`\r` 处理。
 *
 * 为什么每个探测用**不同的文件名**：一组探测共用一个会话与一个工作目录，同名会互相覆盖。
 */
function readGroup(): Group {
  const three = "1\n2\n3\n";
  return {
    tool: "Read",
    name: "read",
    probes: [
      {
        label: "offset-1",
        seed: { "o1.txt": three },
        args: { file_path: "${cwd}/o1.txt", offset: 1 },
      },
      {
        label: "offset-3",
        seed: { "o3.txt": three },
        args: { file_path: "${cwd}/o3.txt", offset: 3 },
      },
      {
        label: "offset-4",
        seed: { "o4.txt": three },
        args: { file_path: "${cwd}/o4.txt", offset: 4 },
      },
      {
        label: "offset-5",
        seed: { "o5.txt": three },
        args: { file_path: "${cwd}/o5.txt", offset: 5 },
      },
      {
        label: "offset-1-limit-2",
        seed: { "o6.txt": three },
        args: { file_path: "${cwd}/o6.txt", offset: 1, limit: 2 },
      },
      {
        label: "offset-2-limit-99",
        seed: { "o7.txt": three },
        args: { file_path: "${cwd}/o7.txt", offset: 2, limit: 99 },
      },
      {
        label: "offset-2-limit-1",
        seed: { "o8.txt": three },
        args: { file_path: "${cwd}/o8.txt", offset: 2, limit: 1 },
      },
      // 截断：默认上限由「2500 行 × 60 字符」与「单行 60000 字符」两个方向探。
      {
        label: "big-2500-lines",
        seed: { "big.txt": BIG_LINES },
        args: { file_path: "${cwd}/big.txt" },
      },
      {
        label: "big-2500-limit-3000",
        seed: { "big2.txt": BIG_LINES },
        args: { file_path: "${cwd}/big2.txt", limit: 3000 },
      },
      {
        label: "one-long-line-60000",
        seed: { "long.txt": ONE_LONG_LINE },
        args: { file_path: "${cwd}/long.txt" },
      },
      // 二进制判定：扩展名 vs 内容，两个方向都要探。
      {
        label: "ascii-named-bin",
        seed: { "ascii.bin": "plain ascii text\n" },
        args: { file_path: "${cwd}/ascii.bin" },
      },
      {
        // 注意**不要**把这种文件叫 `nul.txt`：Windows 上 `nul.<任何扩展名>` 是老式 NUL **设备名**，
        // 写进去的字节会被丢弃、读出来是空——那会录到一个假观察（本探针第一版就踩了这个坑）。
        label: "nul-bytes-named-txt",
        seedBytes: { "nulbytes.txt": Buffer.from([0x61, 0x00, 0x62, 0x0a]) },
        args: { file_path: "${cwd}/nulbytes.txt" },
      },
      {
        label: "utf8-named-bin",
        seed: { "u8.bin": "中文行\nsecond\n" },
        args: { file_path: "${cwd}/u8.bin" },
      },
      // `\r` 处理：行尾与行内。
      {
        label: "interior-cr",
        seed: { "cr.txt": "a\rb\nc\n" },
        args: { file_path: "${cwd}/cr.txt" },
      },
      {
        label: "lone-cr-middle",
        seed: { "crm.txt": "x\n\ry\n" },
        args: { file_path: "${cwd}/crm.txt" },
      },
      // 退化输入。
      { label: "single-newline", seed: { "nl.txt": "\n" }, args: { file_path: "${cwd}/nl.txt" } },
      {
        label: "empty-offset-1-limit-1",
        seed: { "e.txt": "" },
        args: { file_path: "${cwd}/e.txt", offset: 1, limit: 1 },
      },
      {
        label: "offset-string",
        seed: { "t.txt": three },
        args: { file_path: "${cwd}/t.txt", offset: "2" },
      },
      { label: "empty-file-path", seed: { "t2.txt": three }, args: { file_path: "" } },
    ],
  };
}

/**
 * **不存在**的工具（`Glob` / `Grep`）被脚本化调用时的结果——用来证明「会话面没有注册它们」，
 * 而不是「注册了但没对外声明」。见文件头结论 1。
 *
 * `Agent` 作为正对照放在同一组里：它**有**注册，所以拿到的是别的错误
 * （例如入参校验失败 / 子代理缺席），而不是 `tool_not_found`。
 */
function absenceGroup(): Group {
  const seed: Record<string, string> = {
    "one.txt": "alpha\nBeta\ngamma alpha\ndelta\n",
    "sub/two.ts": "const a = 1;\n",
  };
  const absent: Probe[] = [
    { label: "glob-star-txt", seed, args: { pattern: "*.txt" } },
    { label: "glob-no-match", seed, args: { pattern: "**/*.zzz" } },
    { label: "grep-content", tool: "Grep", seed, args: { pattern: "alpha", path: "${cwd}" } },
    {
      label: "grep-files-with-matches",
      tool: "Grep",
      seed,
      args: { pattern: "alpha", path: "${cwd}", output_mode: "files_with_matches" },
    },
    {
      label: "grep-count",
      tool: "Grep",
      seed,
      args: { pattern: "alpha", path: "${cwd}", output_mode: "count" },
    },
    { label: "grep-no-match", tool: "Grep", seed, args: { pattern: "zzzzz", path: "${cwd}" } },
  ];
  return { tool: "Glob", name: "absence", probes: absent };
}

/**
 * **截断（token 预算）**：`Read` 的超大文件路径不是按行数截断，而是按 **token 估算**。
 *
 * 已知事实（2026-10-16）：2500 行 × 60 字符（文件 152499 字节）报
 * 「50833 estimated tokens, limit 25000」，而 `152499 / 3 = 50833` **整除**——
 * 所以估算口径很可能是 `文件字节数 / 3`。本组用几组精心挑的尺寸把这条口径钉死：
 *
 * | 探测 | 文件字节 | 字节/3 | 期望 |
 * | --- | --- | --- | --- |
 * | `bytes-74999` | 74999 | 24999 | 未超限 → 全文 |
 * | `bytes-75000` | 75000 | 25000 | 判 `>` 还是 `>=` |
 * | `bytes-75001` | 75001 | 25000（下取整）/ 25001（上取整） | 判估算是否上取整 |
 * | `bytes-75003` | 75003 | 25001 | 超限 → 部分视图 |
 * | `wide-2500x120` | 302499 | 100833 | 第二个「部分视图行数」数据点 |
 * | `slim-2500x60` | 152499 | 50833 | 取**完整**截断提示文案（head 打印 500 字符） |
 */
function truncGroup(): Group {
  return {
    tool: "Read",
    name: "trunc",
    probes: [
      {
        label: "slim-2500x60",
        seed: { "slim.txt": Array.from({ length: 2500 }, () => "z".repeat(60)).join("\n") },
        args: { file_path: "${cwd}/slim.txt" },
      },
      {
        label: "bytes-74999",
        seed: { "b74999.txt": Array.from({ length: 2500 }, () => "z".repeat(29)).join("\n") },
        args: { file_path: "${cwd}/b74999.txt" },
      },
      {
        label: "bytes-75000",
        seed: { "b75000.txt": "z".repeat(75_000) },
        args: { file_path: "${cwd}/b75000.txt" },
      },
      {
        label: "bytes-75001",
        seed: { "b75001.txt": "z".repeat(75_001) },
        args: { file_path: "${cwd}/b75001.txt" },
      },
      {
        label: "bytes-75003",
        seed: { "b75003.txt": "z".repeat(75_003) },
        args: { file_path: "${cwd}/b75003.txt" },
      },
      {
        label: "wide-2500x120",
        seed: { "wide.txt": Array.from({ length: 2500 }, () => "z".repeat(120)).join("\n") },
        args: { file_path: "${cwd}/wide.txt" },
      },
      {
        label: "slim-limit-1000",
        seed: { "slim2.txt": Array.from({ length: 2500 }, () => "z".repeat(60)).join("\n") },
        args: { file_path: "${cwd}/slim2.txt", limit: 1000 },
      },
      {
        label: "slim-offset-1000-limit-500",
        seed: { "slim3.txt": Array.from({ length: 2500 }, () => "z".repeat(60)).join("\n") },
        args: { file_path: "${cwd}/slim3.txt", offset: 1000, limit: 500 },
      },
    ],
  };
}

/**
 * 收尾问题（只问「实现时非它不可」的几条），一次性跑完：
 *
 * 1. **256KB 文件大小上限**与显式 `limit` 的先后：300KB 文件 + `limit: 5`。
 * 2. **显式 `limit` 与隐式默认的差别**：152499 字节（50833 tok）文件 + `offset:1, limit:2000`
 *    ——隐式默认同样选 2000 行却给「部分视图」，显式给的是「错误」，所以要问清这条边界。
 * 3. **入参校验的错误形状**：`offset: -1`、`limit: 1.5`（`limit: 0` 已知是 ZodError）。
 * 4. **二进制判定是否看扩展名大小写**：`.PNG`。
 * 5. **只有 `\r` 没有 `\n` 的文件**如何分行。
 */
function limitEdgeGroup(): Group {
  const slim = Array.from({ length: 2500 }, () => "z".repeat(60)).join("\n");
  const huge = Array.from({ length: 5000 }, () => "z".repeat(60)).join("\n");
  return {
    tool: "Read",
    name: "limit-edge",
    probes: [
      {
        label: "300KB-with-limit-5",
        seed: { "huge.txt": huge },
        args: { file_path: "${cwd}/huge.txt", limit: 5 },
      },
      {
        label: "50833tok-explicit-limit-2000",
        seed: { "s2000.txt": slim },
        args: { file_path: "${cwd}/s2000.txt", offset: 1, limit: 2000 },
      },
      {
        label: "offset-minus-1",
        seed: { "v1.txt": "1\n2\n3\n" },
        args: { file_path: "${cwd}/v1.txt", offset: -1 },
      },
      {
        label: "limit-fraction",
        seed: { "v2.txt": "1\n2\n3\n" },
        args: { file_path: "${cwd}/v2.txt", limit: 1.5 },
      },
      {
        label: "uppercase-png",
        seed: { "P.PNG": "not really a png\n" },
        args: { file_path: "${cwd}/P.PNG" },
      },
      { label: "only-cr", seed: { "cr.txt": "a\rb\rc" }, args: { file_path: "${cwd}/cr.txt" } },
      // 判「显式」的判据是 `limit` 还是 `offset|limit`，以及两个上限（256KB / 25000 tokens）
      // 是作用在**文件**还是**被选中的范围**上——这三条把它们钉死。
      {
        label: "50733tok-offset-only-2",
        seed: { "s-offset.txt": slim },
        args: { file_path: "${cwd}/s-offset.txt", offset: 2 },
      },
      {
        label: "304999B-implicit-vs-limit-5000",
        seed: { "h-full.txt": huge },
        args: { file_path: "${cwd}/h-full.txt", limit: 5000 },
      },
      {
        label: "302499B-limit-2000",
        seed: { "w-2000.txt": Array.from({ length: 2500 }, () => "z".repeat(120)).join("\n") },
        args: { file_path: "${cwd}/w-2000.txt", limit: 2000 },
      },
    ],
  };
}

const GROUPS: Record<string, () => Group> = {
  read: readGroup,
  trunc: truncGroup,
  "limit-edge": limitEdgeGroup,
  absence: absenceGroup,
};

/**
 * host 对 `session/requestRuntimePreferences` 的应答（spec §1.3）。
 *
 * 默认用 driver 的应答表。`MODE_PROBE_SEARCH=native` 时把
 * `nativeSearchEnhancementsEnabled` 打开——用它**否证**「打开该偏好就会多出 Glob/Grep」
 * 这个猜测（实测：仍然 21 条，仍然没有 Glob/Grep；语义见文件头结论 3）。
 */
function runtimePreferencesReply(): Record<string, unknown> | undefined {
  if (process.env.MODE_PROBE_SEARCH?.trim() !== "native") return undefined;
  return {
    nativeSearchEnhancementsEnabled: true,
    memoryEnabled: false,
    askUserQuestionAutoResolutionEnabled: true,
    modelContextBudgetStrategy: "preflight-v1",
  };
}

/** `tools` 组：把运行时**对外声明**的工具清单一字不差地打出来，并列出全部反向请求。 */
async function dumpToolDeclarations(): Promise<number> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  const declarations: unknown[] = [];
  const nameLists: string[][] = [];
  const reverseRequests: string[] = [];
  const wanted = new Set(["Read", "Glob", "Grep"]);
  try {
    const session = await startOracleRuntime({
      dataRoot,
      cwd,
      script: [
        { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/x.txt" } }] },
        { text: "ok" },
      ],
      runtimePreferencesReply: runtimePreferencesReply(),
      onRawFrame: (frame) => {
        if (typeof frame.method === "string" && frame.id !== undefined) {
          reverseRequests.push(JSON.stringify(frame));
        }
      },
      onMockRequestBody: (_path, body) => {
        const tools = (body as { tools?: unknown[] }).tools;
        if (!Array.isArray(tools)) return;
        const names = tools.map((tool) => String((tool as { name?: string }).name ?? ""));
        nameLists.push(names);
        for (const tool of tools) {
          if (wanted.has((tool as { name?: string }).name ?? "")) declarations.push(tool);
        }
      },
    });
    try {
      await session.send("probe");
      await drain(session.events);
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
  process.stdout.write(
    `\n##### 反向请求（host 必须应答的那一半，${reverseRequests.length} 条）#####\n`,
  );
  for (const frame of reverseRequests) process.stdout.write(`${frame}\n`);
  for (const [index, names] of nameLists.entries()) {
    process.stdout.write(
      `\n##### 请求 #${index} 的工具清单（${names.length} 条）#####\n${names.join(",")}\n`,
    );
  }
  for (const declaration of declarations) {
    process.stdout.write(`\n--- tool declaration ---\n${JSON.stringify(declaration, null, 2)}\n`);
  }
  return declarations.length;
}

async function runGroup(group: Group): Promise<number> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  try {
    const seed: Record<string, string> = {};
    const seedBytes: Record<string, Buffer> = {};
    for (const probe of group.probes) {
      for (const [path, content] of Object.entries(probe.seed ?? {})) {
        if (!(path in seed)) seed[path] = content;
      }
      for (const [path, content] of Object.entries(probe.seedBytes ?? {})) {
        if (!(path in seedBytes)) seedBytes[path] = content;
      }
    }
    await seedWorkspace(cwd, seed);
    for (const [path, content] of Object.entries(seedBytes)) {
      await writeFile(join(cwd, path), content);
    }
    const script: MockStep[] = [
      ...group.probes.map((probe) => ({
        toolCalls: [{ name: probe.tool ?? group.tool, input: probe.args }],
      })),
      { text: `probe ${group.name} done` },
    ];
    const session = await startOracleRuntime({
      dataRoot,
      cwd,
      script,
      runtimePreferencesReply: runtimePreferencesReply(),
    });
    try {
      await session.send(`probe ${group.name}`);
      await drain(session.events);
      process.stdout.write(`\n########## ${group.name} （cwd=${cwd}）##########\n`);
      for (const [index, probe] of group.probes.entries()) {
        const observed = rawToolOutcome(session.frames, index);
        process.stdout.write(`\n== ${index}: ${probe.label} ==\n`);
        process.stdout.write(`args: ${JSON.stringify(probe.args)}\n`);
        if (!observed) {
          process.stdout.write(`(未观察到第 ${index} 次调用的结果)\n`);
          continue;
        }
        process.stdout.write(`ok: ${observed.ok}\n`);
        process.stdout.write(`${describeOutcome(observed)}\n`);
      }
      return group.probes.length;
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

/**
 * 第 `index` 次工具调用的**原始**结果（不经规范化，也不截断）。
 * 配对口径与比较器一致：`scheduled` 的首次出现顺序决定调用序，`toolCallId` 把结果接回去。
 */
function rawToolOutcome(
  frames: readonly OracleEvent[],
  index: number,
):
  | {
      ok: boolean;
      content: string;
      result?: Record<string, unknown>;
      error?: Record<string, unknown>;
    }
  | undefined {
  const order: string[] = [];
  const byId = new Map<
    string,
    {
      ok: boolean;
      content: string;
      result?: Record<string, unknown>;
      error?: Record<string, unknown>;
    }
  >();
  for (const frame of frames) {
    if (frame.type !== "tool.updated") continue;
    const payload = (frame.payload ?? {}) as Record<string, unknown>;
    const id = String(payload.toolCallId ?? "");
    if (payload.kind === "scheduled") {
      order.push(id);
      byId.set(id, { ok: false, content: "" });
      continue;
    }
    const slot = byId.get(id);
    if (!slot) continue;
    if (payload.kind === "result") {
      const result = (payload.result ?? {}) as Record<string, unknown>;
      slot.ok = result.success === true;
      slot.content = String(result.content ?? "");
      slot.result = result;
      continue;
    }
    if (payload.kind === "error") {
      const error = (payload.error ?? {}) as Record<string, unknown>;
      slot.ok = false;
      slot.content = String(error.message ?? "");
      slot.error = error;
    }
  }
  const id = order[index];
  return id === undefined ? undefined : byId.get(id);
}

/** 大块正文只打头尾：探针的输出要能读，而不是把 150KB 倒进终端。 */
function describeOutcome(observed: {
  ok: boolean;
  content: string;
  result?: Record<string, unknown>;
  error?: Record<string, unknown>;
}): string {
  const lines: string[] = [];
  if (observed.result) {
    const result = observed.result;
    lines.push(
      `result: success=${String(result.success)} truncated=${String(result.truncated)} ` +
        `originalBytes=${String(result.originalBytes)} returnedBytes=${String(result.returnedBytes)} ` +
        `budgetStrategy=${String(result.budgetStrategy)}`,
    );
  }
  if (observed.error) {
    // stack 里带机器路径，诊断够用即可；这里保留 message/detail，丢掉 stack。
    const { stack: _stack, ...rest } = observed.error;
    lines.push(`error: ${JSON.stringify(rest)}`);
  }
  lines.push(`contentLength: ${observed.content.length}`);
  if (observed.content.length <= 500) {
    lines.push(`content: ${JSON.stringify(observed.content)}`);
  } else {
    // head 给 500 字符：截断提示文案整条都在头部，短了会看不到它的结尾。
    lines.push(
      `content(head): ${JSON.stringify(observed.content.slice(0, 500))}\n` +
        `content(tail): ${JSON.stringify(observed.content.slice(-300))}`,
    );
  }
  return lines.join("\n");
}

/** 跑到终态为止（探针不关心事件序列，只关心工具结果）。 */
async function drain(events: AsyncIterable<OracleEvent>): Promise<void> {
  const deadline = Date.now() + 120_000;
  for await (const event of events) {
    if (event.type === "turn.completed" || event.type === "turn.failed") return;
    if (Date.now() > deadline) throw new Error("probe：会话没有终态事件");
  }
}

const requested = process.argv.slice(2);
const names = requested.length > 0 ? requested : ["tools", ...Object.keys(GROUPS)];
for (const name of names) {
  if (name === "tools") {
    const count = await dumpToolDeclarations();
    process.stdout.write(`\n##### 工具声明 ${count} 条 #####\n`);
    continue;
  }
  const build = GROUPS[name];
  if (!build) {
    process.stdout.write(`未知组名：${name}\n`);
    continue;
  }
  const probes = await runGroup(build());
  process.stdout.write(`\n##### ${name}: ${probes} 个探针 #####\n`);
}
