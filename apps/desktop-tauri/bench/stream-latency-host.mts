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
 * ## 状态（2026-10-24）：基线已入库
 *
 * 首轮实测（5 turns）：冷 turn 39 ms（含 agent spawn + 存储初始化）；
 * 稳态 **中位 4 ms / p95 5 ms（n=4）**。对照「端到端 p95 ≤ 80 ms」：agent 段
 * （stream-latency.mts，稳态 3–4 ms）+ 本段 ≈ 8–9 ms，余量巨大；WebView 段仍待产品壳 E2E。
 *
 * 接线过程中踩实的坑（本脚本即它们的防回归载体）：
 * 1. `call` 的第二参是**位置参数数组**（ProxyChannel 服务端 `apply(handler, args)`）；
 * 2. Host 自己的 provider 注册表读 `{dataBaseDir}/.mode/v2/provider_config.json`
 *    ——mock 配置必须落数据根规范位置，只给 CLI 注 env 会被判「没有可用的模型供应商」；
 * 3. workspace cwd 必须真实存在（Windows 上 spawn cwd 缺失报成「可执行文件 ENOENT」，
 *    极具误导性）；
 * 4. `listen` 的 arg 不是数组（直接传对象），且 `onDynamicTaskEvent` 是 task 级事件面，
 *    订阅参数必须带真 taskId、每 turn 独立建立；Event 的订阅返回值才是 IDisposable
 *    （把 Event 本身当订阅存起来会在 dispose 时炸）；
 * 5. **数据根必须先初始化出归属文件再起 Host**（`initializeDataRootNonInteractive`）：
 *    否则 CLI 第 2 代把无主 `.mode` 判成 unowned，rename「备份让路」在 Windows 上
 *    撞 Host 未释放的目录句柄 → EPERM → exit 1；这条曾伪装成「node.exe 不存在」级的
 *    假象链，靠 stderr wrapper 才抓到真因；
 * 6. mock 剧本消费按会话内 assistant 消息数推进——每 turn 新 task 恒消费第 0 步，
 *    各步文本必须相同、按顺序配对（按 marker 内容配对会漏配）；
 * 7. 建后立刻发首条输入要 `deferPersistenceUntilFirstPrompt: true`（否则
 *    session_input 外键账本撞 FOREIGN KEY），`sendPrompt` 带 `clientMode:
 *    "desktop-continuous"`（与产品桌面/automation 派发同口径）；
 * 8. mock 配置声明 reasoningLevel 选项档时 session/create 要求显式档位。
 *
 * **诊断工具**：stderr wrapper——Host spawn 的不是真 CLI 而是本脚本生成的 CJS 壳，
 * 壳把 CLI 的 stderr tee 到日志文件（Host 的 protocol-close 清理路径会吞掉 CLI 崩溃栈）。
 *
 * **顺带发现（已修，2026-10-24）**：CLI 的 `turn.started` 事件 payload 带
 * `executionStartedAt` 字段，Host schema 曾判 `unrecognized_keys` 整事件丢弃——
 * 已在 `modeTurnStartedEventPayloadSchema` 补声明（浮点 epoch 毫秒可选键），
 * 本 bench 的丢弃日志清零即其端到端证据；回归
 * `packages/shared/test/mode-protocol-turn-started.test.ts`。
 *
 * 扩展名 `.mts` 与既有 bench/探针同理由：不进 tsconfig include（被 include 拉进来的
 * driver/fixtures 从未被任何 tsc 程序覆盖，会暴露与本 bench 无关的既有类型缺口）。
 */
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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
import { initializeDataRootNonInteractive } from "../../../packages/services/src/data-root/initializer.ts";

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

  // stderr 诊断 wrapper（不改生产代码）：Host spawn 的不是真 CLI 而是这个 CJS 壳，
  // 壳再 spawn 真 CLI 并把子进程 stderr tee 到日志文件 + 透传给自己继承的 stderr。
  // 动机（2026-10-24 阻塞）：agent 第 2 代 ~2s 内 stdout_closed 时，CLI 自身的崩溃栈
  // 被 Host 的 protocol-close 清理路径吞掉（unexpected 日志只在 terminationKind=
  // unexpected 时带 stderrTail）——wrapper 让 stderr 落到一个 Host 管不到的文件里。
  const agentStderrLog = join(workRoot, "agent-stderr.log");
  const wrapperPath = join(workRoot, "agent-stderr-wrapper.cjs");
  writeFileSync(
    wrapperPath,
    [
      '"use strict";',
      'const { spawn } = require("node:child_process");',
      'const fs = require("node:fs");',
      'const logPath = process.env.MODE_BENCH_AGENT_STDERR_LOG;',
      'function log(text) {',
      "  if (!logPath) return;",
      "  try { fs.appendFileSync(logPath, text); } catch {}",
      "}",
      '// argv: [tsxCli, cliEntry, ...cliArgs]——CLI 由 Host 的 MODE_AGENT_SERVER_ARGS_JSON 决定。',
      "const child = spawn(process.execPath, process.argv.slice(2), {",
      '  stdio: ["inherit", "inherit", "pipe"],',
      "  env: process.env,",
      "});",
      'child.stderr.on("data", (chunk) => {',
      "  log(chunk.toString());",
      "  process.stderr.write(chunk);",
      "});",
      'child.on("exit", (code, signal) => {',
      "  log(`\\n[wrapper] exit code=${code} signal=${signal}\\n`);",
      "  process.exit(code == null ? 1 : code);",
      "});",
    ].join("\n"),
    "utf8",
  );
  const watchdog = setTimeout(() => {
    console.error("[stream-latency-host] 看门狗到时（300s）——按失败收场");
    process.exit(1);
  }, 300_000);

  const sentAt: number[] = [];
  const receivedAt: number[] = [];
  // 数据根必须**先初始化出归属文件**再起 Host（与数据根演练测试同一顺序语义）：
  // 否则 Host 会隐式创建无归属的 `.mode`，后续 CLI 第 2 代按 unowned 判定尝试
  // 「备份让路」rename——Windows 上 Host 还持有目录句柄，rename 必 EPERM，
  // CLI exit 1（2026-10-24 wrapper 抓到的真实死因：`.mode` → `.mode.unowned-*` EPERM）。
  const dataRootInit = await initializeDataRootNonInteractive({
    baseDir: dataBaseDir,
    createdBy: "desktop",
    appVersion: "0.0.0-bench",
    action: "fresh",
  });
  console.log(`[stream-latency-host] 数据根初始化：${dataRootInit.state}`);
  const mock = await startMockProvider(
    // 每 turn 一个新 task = 新会话 → 请求里 assistant 消息数恒 0 → 剧本永远消费
    // 第 0 步。所以各步文本必须相同，按**顺序**配对 sentAt/receivedAt（turn 串行，
    // 每 turn 恰一个 delta），不能按 marker 内容区分（首版按 marker 配对，
    // turn 1 的 delta 文本是 step0 的内容，实测漏配）。
    Array.from({ length: turns }, () => ({ text: "host-latency-sample" })),
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
      // 经 stderr wrapper 间接起 CLI：wrapper 负责把 CLI 的 stderr 落盘（见上）。
      MODE_AGENT_SERVER_ARGS_JSON: JSON.stringify([wrapperPath, TSX_CLI, CLI_ENTRY, "app-server", "--stdio"]),
      MODE_BENCH_AGENT_STDERR_LOG: agentStderrLog,
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
      console.log(`[stream-latency-host] turn ${String(turn)}：createTask …`);
      const created = (await withStepTimeout(
        taskChannel.call("createTask", [
          {
            workspacePath,
            modelSelection: {
              providerId: ORACLE_MOCK_PROVIDER_ID,
              modelId: ORACLE_MOCK_MODEL_ID,
              // mock 配置声明了 reasoningLevel 选项档 → session/create 要求显式档位
              // （实测缺省直接拒：`Reasoning level is required`）；延迟基准用 disabled。
              options: { reasoningLevel: "disabled" },
            },
            // 建后立刻发首条输入的无界面形态：用 deferred，让输入 admission 先统一
            // 持久化 session 主记录再写 session_input 外键账本（实测缺省路径直接
            // sendPrompt 会撞 `FOREIGN KEY constraint failed — session=…`）。
            deferPersistenceUntilFirstPrompt: true,
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

      // 等本 turn 终态（task_complete / task_error），保证 turn 边界干净。
      // 记录本 turn 收到的事件 type 序列：超时时把它带进错误消息——「没事件」与
      // 「有事件但没终态」是两种完全不同的故障，不能靠猜。
      const seenTypes: string[] = [];
      let chunkCaptured = false;
      const turnDone = new Promise<void>((resolveTurn, rejectTurn) => {
        const timer = setTimeout(() => {
          rejectTurn(
            new Error(
              `turn ${String(turn)} 60s 内未到终态；已见事件：` +
                `${seenTypes.slice(0, 20).join(", ") || "（无）"}`,
            ),
          );
        }, 60_000);
        subscriptions.push(
          turnEvents((payload) => {
            seenTypes.push(`${payload.type}:${payload.taskId ?? "-"}`);
            if (payload.taskId !== created.taskId) return;
            const at = Date.now();
            // 每 turn 恰一个 text delta：只记首个 chunk（顺序配对，见 mock 剧本注释）。
            if (!chunkCaptured && payload.type === "agent_message_chunk") {
              chunkCaptured = true;
              receivedAt.push(at);
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
            // 与产品桌面路径同口径（automationRuntime 派发同款）。
            clientMode: "desktop-continuous",
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
    // wrapper 落盘的 CLI stderr（Host 管不到的旁路）——gen2 死因就在这里。
    if (existsSync(agentStderrLog)) {
      const agentLog = readFileSync(agentStderrLog, "utf8");
      if (agentLog.trim().length > 0) {
        console.error(`[stream-latency-host] CLI stderr（wrapper 落盘）：\n${agentLog}`);
      }
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
