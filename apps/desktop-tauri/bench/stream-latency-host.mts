/**
 * 流式增量延迟基线——**Host 段**（M4a 性能四件 stream-latency 的第二段）。
 *
 * ## 测量面（混用即无效）
 *
 * 「模型端点（本地 mock provider）写出 SSE delta」→「mode-task 频道的订阅者
 * （Node ChannelClient，与 renderer 同一条 RPC 链路）收到 `agent_message_chunk`」。
 * 覆盖：mock HTTP → 真 CLI agent（`app-server --stdio`，由真 Host 按
 * `MODE_AGENT_SERVER_COMMAND` spawn）→ mode-protocol NDJSON → Host services 的
 * adapter 映射（`modeTaskServiceAdapter`）→ ChannelServer → stdio 帧 → 客户端解码。
 *
 * **不在本测量面内**（如实声明）：
 * - 壳桥（Rust）与 WebView MessagePort 段——那半截需要产品壳 E2E，待做；
 * - 真实模型首 token（网络与模型侧成本，与本机转发无关）；
 * - 后台任务的 1.5s coalescer 合并（前台 task 不经它）。
 *
 * 与 `bench/stream-latency.mts`（agent 段）的差值 ≈ Host 转发段的增量。
 *
 * ## 统计口径（先于数字写定）
 *
 * - 剧本 N 个纯文本步（每 turn 一步一 delta）；默认 N=5，`--turns <n>` 可覆盖；
 *   首 turn 含 agent 冷 spawn，单列不进中位（与冷启动脚本同口径）；
 * - 每 turn 一个样本：`latency = t(客户端收到首个匹配 delta 的 chunk) − t(mock 写出 delta)`；
 *   两端都打点在**本 bench 进程**（mock server 与 ChannelClient 同进程），`Date.now()` 可比；
 * - 报 min / 中位 / p95 / max；对照「端到端 p95 ≤ 80 ms」时这是下界组成段的 Host 半截。
 *
 * ```bash
 * pnpm --filter @mode/desktop-tauri measure:stream-latency-host
 * node --import tsx apps/desktop-tauri/bench/stream-latency-host.mts --turns 8
 * ```
 *
 * ## 状态（2026-10-24）：opt-in WIP，被 agent 二次生成启动失败阻塞
 *
 * 接线已走通的部分（本脚本即这四条坑的复现/防回归载体）：
 * 1. `call` 的第二参是**位置参数数组**（ProxyChannel 服务端 `apply(handler, args)`）；
 * 2. Host 自己的 provider 注册表读 `{dataBaseDir}/.mode/v2/provider_config.json`
 *    ——mock 配置必须落数据根规范位置，只给 CLI 注 env 会被判「没有可用的模型供应商」；
 * 3. workspace cwd 必须真实存在（Windows 上 spawn cwd 缺失报成「可执行文件 ENOENT」，
 *    极具误导性）；
 * 4. `listen` 的 arg 不是数组（直接传对象），且 `onDynamicTaskEvent` 是 task 级事件面，
 *    订阅参数必须带真 taskId、每 turn 独立建立。
 *
 * **阻塞卡点**：`agent.initialize`（agent 第 1 代）成功，但 `createTask` 触发的
 * agent 第 2 代进程 ~2s 内 `stdout_closed`（Host 只见 protocol-close；CLI 自身 stderr
 * 被吞——`Mode agent process exited unexpectedly` 日志只在 terminationKind=unexpected
 * 时携带 stderr，protocol-close 路径不打）。已排除的口径：
 * - 给 `MODE_HOST_RESOURCES_DIR`（打包形态）→ initialize 反而挂死
 *   （libuv `fs-event.c:72` 断言 + 90s 超时）；
 * - 最小 env（剔除开发机全局 MODE_*）→ 同样失败；
 * - 手动以相同 args/cwd/env 直跑 CLI → 正常（差异在 Host spawn 的 spawnEnv 细节）。
 *
 * **下一步建议**：在 `modeAgentProcessManager` 的 protocol-close 清理路径临时打印
 * stderrTail（或用 ProcMon/调试器挂 CLI 子进程），定位 gen2 的真实退出原因；
 * 本脚本修通后即可产出 Host 段基线数字。
 *
 * 扩展名 `.mts` 与既有 bench/探针同理由：不进 tsconfig include（被 include 拉进来的
 * driver/fixtures 从未被任何 tsc 程序覆盖，会暴露与本 bench 无关的既有类型缺口）。
 */
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import type { IChannel } from "@mode/rpc";

import {
  connectProductionHost,
  disposeHost,
} from "../test/fixtures/productionHost.mts";
import {
  ORACLE_MOCK_MODEL_ID,
  ORACLE_MOCK_PROVIDER_ID,
  startMockProvider,
  writeOracleProviderConfig,
} from "../../../packages/desktop/tests/agent-runtime-oracle/mock-provider.mts";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const TSX_CLI = join(REPO_ROOT, "node_modules", "tsx", "dist", "cli.mjs");
const CLI_ENTRY = join(REPO_ROOT, "apps", "mode-cli", "packages", "cli", "src", "main.ts");
const BUILTIN_PROVIDER_CONFIG = join(REPO_ROOT, "config", "provider", "mode-builtin.json");

function parseTurns(argv: string[]): number {
  const index = argv.indexOf("--turns");
  if (index < 0) return 5;
  const value = Number(argv[index + 1]);
  if (!Number.isInteger(value) || value < 1 || value > 50) {
    throw new Error("--turns 必须是 1..50 的整数（每 turn 一次完整 agent 回合，别开太大）");
  }
  return value;
}

function percentile(sorted: number[], p: number): number {
  // 最接近 p% 的样本（ceil），不插值：小样本插值会造出没测过的数。
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

interface Disposable {
  dispose(): void;
}

/** 每步限时：看门狗只能兜全局，卡在哪一步要从错误消息直接看出来。 */
async function withStepTimeout<T>(promise: Promise<T>, ms: number, step: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(`步骤 ${step} 超时（${String(ms / 1000)}s）`)), ms);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function main(): Promise<void> {
  const turns = parseTurns(process.argv.slice(2));
  const workRoot = mkdtempSync(join(tmpdir(), "mode-stream-latency-host-"));
  const dataBaseDir = join(workRoot, "data");
  const home = join(workRoot, "home");
  const workspacePath = join(workRoot, "workspace");
  // workspace 必须真实存在：spawn 的 cwd 缺失在 Windows 上会报成
  // 「可执行文件 ENOENT」（实测报 node.exe 不存在，极具误导性）。
  mkdirSync(workspacePath, { recursive: true });
  const personalConfigPath = join(workRoot, "oracle-personal-provider.json");
  const watchdog = setTimeout(() => {
    console.error("[stream-latency-host] 看门狗到时（300s）——按失败收场");
    process.exit(1);
  }, 300_000);

  const sentAt: number[] = [];
  const receivedAt: number[] = [];
  const receivedContents: string[] = [];
  const mock = await startMockProvider(
    Array.from({ length: turns }, (_, index) => ({
      text: `host-latency-sample-${String(index)}`,
    })),
    {
      onContentBlockDeltaWrite: (at) => {
        sentAt.push(at);
      },
    },
  );
  await writeOracleProviderConfig(personalConfigPath, mock.baseUrl);
  // Host 自己的 provider 注册表读 `{dataBaseDir}/.mode/v2/provider_config.json`
  // （`node.ts` 的 `resolveAppConfigDir()` + `PERSONAL_PROVIDER_CONFIG_FILE_NAME`），
  // 而 agent spawn env 里的 MODE_PERSONAL_PROVIDER_CONFIG_FILE 也被 Host 重写为同一路径
  // （`createNodeProviderRuntimePathEnv`）——所以 mock 配置必须落在数据根的规范位置；
  // 只给 CLI 注 env 的旧口径实测会被 Host 的注册表判成「没有可用的模型供应商」。
  const hostProviderConfigDir = join(dataBaseDir, ".mode", "v2");
  mkdirSync(hostProviderConfigDir, { recursive: true });
  writeFileSync(
    join(hostProviderConfigDir, "provider_config.json"),
    // 直接复用 oracle 写出的同一份配置（读回来再写一次，避免两处剧本漂移）。
    readFileSync(personalConfigPath, "utf8"),
    "utf8",
  );

  // Host 的 agent spawn env 会用 providerConfigRuntime 物化后的 builtin 路径**覆盖**
  // MODE_BUILTIN_PROVIDER_CONFIG_FILE；给 MODE_HOST_RESOURCES_DIR 又会让 initialize
  // 挂死（libuv fs-event 断言 + 90s 超时，实测）——两个口径都试过，均未走通，
  // 见文件头「已知阻塞」。
  //
  // 最后一试：**最小 env**。夹具默认继承 process.env——开发机上的全局 MODE_* 变量
  // （本机实测有 MODE_DATA_BASE_DIR 等）会随 Host → agent 透传，可能触发 CLI 的
  // dev-only 分支；oracle driver 之所以稳定，正是它只给显式构造的最小 env。
  const host = await connectProductionHost({
    home,
    dataBaseDir,
    startupId: "stream-latency-host",
    env: {
      PATH: process.env.PATH,
      SYSTEMROOT: process.env.SYSTEMROOT,
      TEMP: process.env.TEMP,
      TMP: process.env.TMP,
      PATHEXT: process.env.PATHEXT,
      SystemDrive: process.env.SystemDrive,
      COMSPEC: process.env.COMSPEC,
    },
    extraEnv: {
      // Host 按这两个变量 spawn agent；指向真 CLI（tsx 直跑源码，与 oracle driver 同口径）。
      MODE_AGENT_SERVER_COMMAND: process.execPath,
      MODE_AGENT_SERVER_ARGS_JSON: JSON.stringify([TSX_CLI, CLI_ENTRY, "app-server", "--stdio"]),
      MODE_BUILTIN_PROVIDER_CONFIG_FILE: BUILTIN_PROVIDER_CONFIG,
      MODE_PERSONAL_PROVIDER_CONFIG_FILE: join(hostProviderConfigDir, "provider_config.json"),
    },
  });

  const subscriptions: Disposable[] = [];
  try {
    const agentChannel = host.client.getChannel<IChannel>("mode-agent");
    // 与产品渲染层同序：先 initialize（等 CLI 存储启动与握手），再建任务。
    // 注意：`call` 的第二参是**位置参数数组**（ProxyChannel 服务端 `apply(handler, args)`），
    // 传单对象会让方法以零参调用（首跑实测：读 undefined.workspaceIdentity 当场炸）。
    console.log("[stream-latency-host] 步骤：agent.initialize …");
    await withStepTimeout(agentChannel.call("initialize", [{ workspacePath }]), 90_000, "agent.initialize");
    console.log("[stream-latency-host] 步骤：agent.initialize 完成");

    const taskChannel = host.client.getChannel<IChannel>("mode-task");

    for (let turn = 0; turn < turns; turn += 1) {
      const marker = `host-latency-sample-${String(turn)}`;
      console.log(`[stream-latency-host] turn ${String(turn)}：createTask …`);
      const created = (await withStepTimeout(
        taskChannel.call("createTask", [
          {
            workspacePath,
            modelSelection: { providerId: ORACLE_MOCK_PROVIDER_ID, modelId: ORACLE_MOCK_MODEL_ID },
          },
        ]),
        60_000,
        `turn ${String(turn)} createTask`,
      )) as { taskId: string; traceId: string };
      console.log(`[stream-latency-host] turn ${String(turn)}：taskId=${created.taskId}，sendPrompt …`);

      // onDynamicTaskEvent 是 task 级事件面：订阅 arg 必须带真 taskId，
      // 每 turn 用正确参数独立建立（固定参数的订阅收不到该 task 的事件）。
      const turnEvents = taskChannel.listen<{
        type: string;
        content?: string;
        taskId?: string;
      }>("onDynamicTaskEvent", { workspacePath, taskId: created.taskId });
      subscriptions.push(turnEvents);

      // 等本 turn 终态（task_complete / task_error），保证 turn 边界干净。
      const turnDone = new Promise<void>((resolveTurn, rejectTurn) => {
        const timer = setTimeout(() => {
          rejectTurn(new Error(`turn ${String(turn)} 60s 内未到终态`));
        }, 60_000);
        subscriptions.push(
          turnEvents((payload) => {
            if (payload.taskId !== created.taskId) return;
            const at = Date.now();
            if (payload.type === "agent_message_chunk" && typeof payload.content === "string") {
              if (payload.content.includes(marker)) {
                receivedAt.push(at);
                receivedContents.push(payload.content);
              }
            }
            if (payload.type === "task_complete" || payload.type === "task_error") {
              clearTimeout(timer);
              resolveTurn();
            }
          }),
        );
      });

      await withStepTimeout(
        taskChannel.call("sendPrompt", [
          {
            taskId: created.taskId,
            traceId: created.traceId,
            content: `第 ${String(turn + 1)} 轮 Host 段流式样本，直接回一句话即可。`,
          },
        ]),
        30_000,
        `turn ${String(turn)} sendPrompt`,
      );
      await turnDone;
      console.log(
        `[stream-latency-host] turn ${String(turn)}：终态已到（mock delta ${String(sentAt.length)} / 客户端 chunk ${String(receivedAt.length)}）`,
      );
    }

    if (sentAt.length !== turns || receivedAt.length !== turns) {
      throw new Error(
        `样本配对失败：mock 写出 ${String(sentAt.length)} 个 delta，` +
          `客户端收到 ${String(receivedAt.length)} 个匹配 chunk（期望各 ${String(turns)}）`,
      );
    }
    const allLatencies = sentAt.map((sent, index) => (receivedAt[index] as number) - sent);
    for (let turn = 0; turn < turns; turn += 1) {
      console.log(
        `[stream-latency-host] turn ${String(turn)}：SSE 写出 → mode-task 订阅者收到 ` +
          `${String(allLatencies[turn])} ms${turn === 0 ? "（冷 turn：含 agent spawn）" : ""}`,
      );
    }
    const warm = allLatencies.slice(1);
    const basis = warm.length > 0 ? warm : allLatencies;
    console.log(
      `[stream-latency-host] Host 段（mock SSE → mode-task 频道客户端）：` +
        `${describe(basis)}${warm.length > 0 ? "（已去掉冷 turn）" : "（n=1，全量）"}`,
    );
    console.log(
      "[stream-latency-host] 不含壳桥与 WebView 段（见文件头「测量面」）；" +
        "与 agent 段 bench（stream-latency.mts）的差值 ≈ Host 转发增量。",
    );
  } finally {
    clearTimeout(watchdog);
    for (const subscription of subscriptions) {
      subscription.dispose();
    }
    // 失败诊断：Host stderr 尾部是卡点的唯一现场（agent spawn 报错都打在这里）。
    const stderrTail = host.stderr().split(/\r?\n/).slice(-60).join("\n");
    if (stderrTail.trim().length > 0) {
      console.error(`[stream-latency-host] Host stderr 尾部：\n${stderrTail}`);
    }
    await disposeHost(host).catch(() => undefined);
    await mock.close();
    rmSync(workRoot, { recursive: true, force: true });
  }
}

main().catch((error: unknown) => {
  console.error(
    `[stream-latency-host] 失败：${error instanceof Error ? error.message : String(error)}`,
  );
  process.exitCode = 1;
});
