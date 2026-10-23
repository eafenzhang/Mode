/**
 * 流式增量延迟基线（M4a 性能四件之四，口径见路线图「度量与预算」节）。
 *
 * ## 测量面（混用即无效）
 *
 * 测的是** agent 运行时内部段**：模型端点（本地 mock provider）写出一个 SSE
 * `content_block_delta`，到 agent 进程（真 `app-server --stdio`）在 stdout 上吐出对应
 * `session/event model.streaming kind=text_delta` NDJSON 帧——覆盖 HTTP 消费、SSE 解析、
 * 事件映射、NDJSON 序列化与管道传输。
 *
 * **不在本测量面内**（如实声明，不假装覆盖）：
 * - Host services 转发与 ChannelServer 分发（前台会话不经 coalescer，见
 *   `modeSessionEventCoalescer.ts` 的 Background 命名；后台合并是另一个人工可测面）；
 * - 壳桥（stdio 帧编解码已由 `ipc-throughput` 覆盖）与 WebView MessagePort 段；
 * - 真实模型的首 token 时间（那是网络与模型侧成本，与本机转发无关）。
 *
 * ## 统计口径（先于数字写定）
 *
 * - 剧本：N 个纯文本步（每 turn 一步、一步一个 delta）；默认 N=10，`--turns <n>` 可覆盖；
 * - 每 turn 一个样本：`latency = t(收到 text_delta 帧) − t(mock 写出 delta)`；
 *   两端都在**本 bench 进程**内打点（mock server 与 driver stdout 解析同进程），
 *   `Date.now()` 可比；
 * - 报 min / 中位 / p95 / max（n<20 时 p95 取排序后最接近 95% 的样本，不插值）；
 * - 驱动复用 oracle 的 `startOracleRuntime`（隔离数据根 + 真 CLI + 本地 mock），
 *   不碰真实模型、不产生计费。
 *
 * ```bash
 * pnpm --filter @mode/desktop-tauri measure:stream-latency
 * node --import tsx apps/desktop-tauri/bench/stream-latency.mts --turns 20
 * ```
 *
 * 扩展名是 `.mts`：与 oracle 探针同一跑法（tsx 直跑），刻意不进
 * `apps/desktop-tauri/tsconfig.json` 的 `include`——被 include 拉进 tsc 的是
 * `driver.mts` 全文，而它从未被任何 tsc 程序覆盖（desktop 无 typecheck 脚本），
 * 拉进来会暴露与本 bench 无关的既有类型缺口。
 */
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "../../../packages/desktop/tests/agent-runtime-oracle/driver.mts";

interface Sample {
  turn: number;
  latencyMs: number;
}

function parseTurns(argv: string[]): number {
  const index = argv.indexOf("--turns");
  if (index < 0) return 10;
  const value = Number(argv[index + 1]);
  if (!Number.isInteger(value) || value < 1 || value > 200) {
    throw new Error("--turns 必须是 1..200 的整数");
  }
  return value;
}

function percentile(sorted: number[], p: number): number {
  // 最接近 p% 的样本（ceil），不插值：样本量小的时候插值会造出没测过的数。
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(Math.max(rank, 1), sorted.length) - 1] as number;
}

function describe(values: number[]): string {
  const sorted = [...values].sort((left, right) => left - right);
  return (
    `min ${String(sorted[0])} ms，中位 ${String(percentile(sorted, 50))} ms，` +
    `p95 ${String(percentile(sorted, 95))} ms，max ${String(sorted[sorted.length - 1])} ms` +
    `（n=${String(sorted.length)}）`
  );
}

async function main(): Promise<void> {
  const turns = parseTurns(process.argv.slice(2));
  const dataRoot = mkdtempSync(join(tmpdir(), "mode-stream-latency-"));
  const cwd = mkdtempSync(join(tmpdir(), "mode-stream-latency-cwd-"));

  const sentAt: number[] = [];
  const receivedAt: number[] = [];
  const watchdog = setTimeout(() => {
    console.error("[stream-latency] 看门狗到时（120s）——turn 未按预期完成，按失败收场");
    process.exit(1);
  }, 120_000);

  const session = await startOracleRuntime({
    dataRoot,
    cwd,
    // 每 turn 消费一步（scriptIndex = 请求里 assistant 消息数），文本各不相同便于诊断。
    script: Array.from({ length: turns }, (_, index) => ({
      text: `stream-latency-sample-${String(index)}`,
    })),
    onMockContentBlockDeltaWrite: (at) => {
      sentAt.push(at);
    },
    onRawFrame: (frame) => {
      if (frame.method !== "session/event") return;
      const params = (frame.params ?? {}) as Record<string, unknown>;
      if (params.type !== "model.streaming") return;
      const payload = (params.payload ?? {}) as Record<string, unknown>;
      if (payload.kind !== "text_delta") return;
      receivedAt.push(Date.now());
    },
  });

  try {
    for (let turn = 0; turn < turns; turn += 1) {
      await session.send(`第 ${String(turn + 1)} 轮流式样本，直接回一句话即可。`);
      // 消费到本 turn 终态；迭代器的 return() 只结束本次 for-await，下一轮重新开迭代器。
      for await (const event of session.events) {
        if (event.type === "turn.completed") break;
        if (event.type === "turn.failed") {
          throw new Error(`turn.failed：${JSON.stringify(event.payload)}`);
        }
      }
    }

    if (sentAt.length !== turns || receivedAt.length !== turns) {
      throw new Error(
        `样本配对失败：mock 写出 ${String(sentAt.length)} 个 delta，` +
          `agent 吐出 ${String(receivedAt.length)} 个 text_delta 帧（期望各 ${String(turns)}）`,
      );
    }
    const samples: Sample[] = sentAt.map((sent, index) => ({
      turn: index,
      latencyMs: (receivedAt[index] as number) - sent,
    }));
    for (const sample of samples) {
      console.log(
        `[stream-latency] turn ${String(sample.turn)}：SSE 写出 → agent 帧出口 ` +
          `${String(sample.latencyMs)} ms`,
      );
    }
    console.log(
      `[stream-latency] agent 段（mock SSE → stdout text_delta 帧）：` +
        describe(samples.map((sample) => sample.latencyMs)),
    );
    console.log(
      "[stream-latency] 不含 Host 转发与 WebView 段（见文件头「测量面」）；" +
        "对照目标「端到端 p95 ≤ 80 ms」时这是下界组成段。",
    );
  } finally {
    clearTimeout(watchdog);
    await session.stop();
    rmSync(dataRoot, { recursive: true, force: true });
    rmSync(cwd, { recursive: true, force: true });
  }
}

main().catch((error: unknown) => {
  console.error(`[stream-latency] 失败：${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
