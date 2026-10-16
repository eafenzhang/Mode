/**
 * M3 Task 2 · Step 4 的**自动化那一半**：真启动失败时，Host 必须把 `failed` + `errorCode`
 * 报出来并退出，**而不是**半可用地活下去。
 *
 * 落点说明：本用例与 `packages/desktop/tests/host-standalone.test.mts` 同源（都是「Host 入口
 * 自己的契约」），所以放在这棵树上——发布流程的 verify job 跑 `packages/desktop/tests/*.test.mts`，
 * 它进 CI；`@mode/shared` 在这里可解析（`apps/desktop-tauri` 只依赖 `@mode/rpc`，那里要拿
 * 渲染层词汇表就得新增依赖，代价大于收益）。
 *
 * ## 这条守的是什么（为什么它不是「再来一条用例」）
 *
 * 渲染层的启动计时器**一收到任何状态就被拆除**（`packages/desktop/src/renderer/src/main.tsx`；
 * 复评 I4(b) 纠正过这个口径）。所以一旦壳把 `starting` 投出去，渲染层就再没有自己的超时了——
 * **此后唯一的失败信号就是壳推的 `failed` + `errorCode`**（DR-T1-3）。而壳推 `failed` 的判据是
 * 「Host 就绪前退出」（`crates/sidecar-host` 的 reaper → `failed(transport_closed)`，由
 * `crates/sidecar-host/tests/host_lifecycle.rs` 覆盖）。两半之间的**接缝**在这里：
 *
 * - Host 侧必须真的**退出**（退出码非 0），且失败前**不写任何帧**——壳的就绪判据是「第一帧」，
 *   所以「失败时没有帧」正是壳能把这次失败读成失败（而不是读成就绪）的前提；
 * - 报出来的错码必须是**渲染层词汇表里的值**（`databaseStartupErrorCodeSchema`）：写一个
 *   schema 之外的值，渲染层会因 `.strict()` 把**整条状态消息**丢掉，症状是「失败屏都不画、
 *   只等自己那个早就被拆掉的计时器」——最难查的一类失败；
 * - 失败要说出**停在哪个应用阶段**（`failedPhase`），因为渲染层的重试/退出判据读它。
 *
 * ## 怎么把失败造得确定
 *
 * 让数据**基**目录的位置上是一个**文件**：真存储起来时 `mkdir {base}` / 打开 sqlite 必然
 * `ENOTDIR`。实测（本文件就是那条实测的固化，见 spec §10 的证据登记）：
 *
 * ```text
 * [mode-host-sidecar] startup_failed phase=failed failedPhase=preparing_host_storage
 *   errorCode=sql_failed error=ENOTDIR: not a directory, mkdir '…\data'
 * ```
 *
 * 它比「换一个假 Host 入口」强的地方：走的是**产品入口的完整启动序列**
 * （原生依赖探针 → 真存储 → 真服务面），失败点在真存储上，正是真实产品会踩的那类
 * （目录被文件占了、父目录不存在、盘符只读）。
 */
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";

import { databaseStartupErrorCodeSchema, databaseStartupPhaseSchema } from "@mode/shared";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
/** 生产 Host 入口：壳 spawn 的就是这一个文件（见 docs/specs/tauri-renderer-host.md）。 */
const HOST_ENTRY = resolve(repoRoot, "packages/desktop/src/host-standalone/index.ts");

const delay = (ms: number): Promise<void> =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

interface FailureRun {
  exitCode: number | null;
  stdout: string;
  stderr: string;
}

/** 起 Host（stdin 保持打开，否则会走「stdin 关闭 → 主动退出」那条正常路径）并等它自己退出。 */
async function runHostUntilExit(options: {
  home: string;
  dataBaseDir: string;
  timeoutMs?: number;
}): Promise<FailureRun> {
  const child = spawn("node", ["--import", "tsx", HOST_ENTRY], {
    cwd: repoRoot,
    env: {
      ...process.env,
      HOME: options.home,
      USERPROFILE: options.home,
      MODE_DATA_BASE_DIR: options.dataBaseDir,
      MODE_HOST_STARTUP_ID: "startup-failure-test",
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk: Buffer) => {
    stdout += chunk.toString();
  });
  child.stderr.on("data", (chunk: Buffer) => {
    stderr += chunk.toString();
  });

  const deadline = Date.now() + (options.timeoutMs ?? 30_000);
  while (child.exitCode === null && child.signalCode === null && Date.now() < deadline) {
    await delay(100);
  }
  const exitCode = child.exitCode;
  if (exitCode === null) child.kill();
  child.stdin.end();
  await delay(100);
  return { exitCode, stdout, stderr };
}

test("真启动失败：Host 报 failed + errorCode、不写任何帧、以非零码退出", async () => {
  const sandbox = mkdtempSync(join(tmpdir(), "mode-host-startup-fail-"));
  const home = join(sandbox, "home");
  // 数据**基**目录的位置上放一个文件：真存储起来时必然失败（ENOTDIR / SQLITE_CANTOPEN）。
  const dataBaseDir = join(sandbox, "data");
  writeFileSync(dataBaseDir, "这不是目录");

  try {
    const run = await runHostUntilExit({ home, dataBaseDir });

    // ① 必须**退出**：壳推 failed 的判据之一是「就绪前退出」（另一条是传输断开）。
    //    Host 若不退出而是带着半可用的服务面活下去，渲染层会永远停在 starting。
    assert.notEqual(run.exitCode, null, `Host 必须在启动失败后退出；stderr:\n${run.stderr}`);
    assert.notEqual(run.exitCode, 0, `启动失败必须以非零码退出；stderr:\n${run.stderr}`);

    // ② 失败前不得写出任何帧：壳的就绪判据是「Host 的第一帧」，此时不该有帧。
    assert.equal(
      run.stdout,
      "",
      `启动失败时不得往 stdout（帧流）写任何字节；实际：${JSON.stringify(run.stdout.slice(0, 200))}`,
    );

    // ③ 结构化留痕里必须同时有应用阶段、失败阶段与错码（这是壳/人排障的唯一入口）。
    const failure = /startup_failed phase=(\S+) failedPhase=(\S+) errorCode=(\S+)/.exec(run.stderr);
    assert.ok(failure, `stderr 里应有结构化的 startup_failed 记录；实际：\n${run.stderr}`);
    const [, phase, failedPhase, errorCode] = failure;
    assert.equal(phase, "failed", "报告的应用阶段必须是 failed");
    assert.equal(
      failedPhase,
      "preparing_host_storage",
      "失败要说出停在哪一步：基目录不可用发生在真存储那一步",
    );

    // ④ 词汇表必须落在**渲染层 schema** 的取值集合里（跨语言契约：写 schema 之外的值会让整条状态被丢）。
    assert.ok(
      databaseStartupErrorCodeSchema.options.includes(errorCode as never),
      `errorCode "${errorCode}" 必须在 databaseStartupErrorCodeSchema 里：` +
        databaseStartupErrorCodeSchema.options.join(","),
    );
    assert.ok(
      databaseStartupPhaseSchema.options.includes(phase as never),
      `phase "${phase}" 必须在 databaseStartupPhaseSchema 里`,
    );

    // ⑤ 失败就是失败：不得在失败之后还报出一句 "ready"（半可用会被读成全可用）。
    assert.doesNotMatch(run.stderr, /\] ready startupId=/, "启动失败后不得报 ready");

    // ⑥ 也没有越过失败点继续起服务（服务面装配在真存储之后，本用例失败在存储上）。
    assert.doesNotMatch(run.stderr, /services_ready/, "失败在存储阶段，不得走到服务面装配");
  } finally {
    rmSync(sandbox, { recursive: true, force: true });
  }
});
