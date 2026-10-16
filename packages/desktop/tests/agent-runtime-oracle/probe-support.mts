// M2 Task 2：只读工具探针的**共用支撑**（诊断用，不进对拍套件）。
//
// 纪律（docs/specs/clean-room-policy.md §黑盒对拍）：只运行旧实现、观察它的输入输出与事件序列。
// 本文件不读旧实现的实现体，也不把观察「手写」进夹具——夹具由 `record.mts` 录。
//
// ## 探针为什么分成了几个文件
//
// oxlint 的 `max-lines` 上限是 400 行（代码行），一个 600+ 行的「探针大全」会让 `pnpm lint`
// 变成**错误**、进而卡住 release 的 verify 步骤。所以按关注点分开：
//
// | 文件 | 关注点 |
// | --- | --- |
// | `probe-tool-surface.mts` | 运行时对外声明了哪些工具（清单 / 反向请求 / 缺哪些工具） |
// | `probe-read.mts` | `Read` 的入参语义与两道上限（行号、offset/limit、截断、部分视图） |
// | `probe-read-history.mts` | `Read` 的**会话内历史**（重复读去重、去重按什么判「变了」） |
// | `probe-support.mts` | 三者共用的类型与打印口径（本文件） |
//
// ## 踩过的坑（留给下一个人）
//
// Windows 上 `nul.<任意扩展名>`（以及 `con`/`aux`/`prn`/`com1`…）是**设备名**：写进去的字节被
// 丢弃、读出来是空。用 `nul.txt` 当「含 NUL 字节的文本文件」会录到一个假观察（第一版就踩了，
// 夹具里已改名 `nulbytes.txt`）。
//
// 另一个同类坑：**同一条对话里「接着 send 一次」不等于新 turn**——运行时的 turn 只在模型给出
// 收尾文本时才结束。所以「跨 turn」要在剧本里插一个文本步，否则第二步的读其实还在第一个 turn 里
// （`probe-read-history.mts` 的剧本因此带 `{text}` 分隔）。
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { seedWorkspace, startOracleRuntime, type OracleEvent } from "./driver.mjs";
import type { MockStep } from "./mock-provider.mjs";

export type Probe = {
  label: string;
  /** 覆盖组默认工具名（`absence` 组要同时探 Glob 与 Grep）。 */
  tool?: string;
  /** UTF-8 种子文件。 */
  seed?: Record<string, string>;
  /** **原始字节**的种子（二进制探测）：`seedWorkspace` 只收 utf8 文本。 */
  seedBytes?: Record<string, Buffer>;
  args: Record<string, unknown>;
};

/** 探针组：一组探测共用一个会话与一个工作目录。 */
export type ProbeGroup = { tool: string; name: string; probes: Probe[] };

/** 暴露 `MockStep`，免得每个探针文件都 import 一次。 */
export type { MockStep };

/**
 * host 对 `session/requestRuntimePreferences` 的应答（spec §1.3）。
 *
 * 默认用 driver 的应答表。`MODE_PROBE_SEARCH=native` 时把
 * `nativeSearchEnhancementsEnabled` 打开——用它**否证**「打开该偏好就会多出 Glob/Grep」这个猜测
 * （实测：仍然 21 条、仍然没有 Glob/Grep；该偏好的语义是「为 Bash 注入 bfs/ugrep 增强」）。
 */
export function runtimePreferencesReply(): Record<string, unknown> | undefined {
  if (process.env.MODE_PROBE_SEARCH?.trim() !== "native") return undefined;
  return {
    nativeSearchEnhancementsEnabled: true,
    memoryEnabled: false,
    askUserQuestionAutoResolutionEnabled: true,
    modelContextBudgetStrategy: "preflight-v1",
  };
}

export async function runGroup(group: ProbeGroup): Promise<number> {
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
export function rawToolOutcome(
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
export function describeOutcome(observed: {
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
export async function drain(events: AsyncIterable<OracleEvent>): Promise<void> {
  const deadline = Date.now() + 120_000;
  for await (const event of events) {
    if (event.type === "turn.completed" || event.type === "turn.failed") return;
    if (Date.now() > deadline) throw new Error("probe：会话没有终态事件");
  }
}
