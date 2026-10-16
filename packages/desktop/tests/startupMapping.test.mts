/**
 * M3 Task 2 · Step 3：`startup/storageState` → `DatabaseStartupState` 的**夹具驱动**翻译测试（DR-T2-2）。
 *
 * 守的是「一层显式翻译」这件事本身：两个词汇表的抽象层级不同（一个库的阶段 vs 整个应用的阶段），
 * 把名字凑成同名会在几处**只有真跑才暴露**的地方出错——例如运行时某个库 `ready` 时应用其实还没
 * 准备好（另一个库与服务面还没起），或者失败时渲染层读的是 `failedPhase` 而不是库内阶段。
 *
 * 四条路径（计划要求）都覆盖：
 *
 * 1. `starting`：状态机初值（还没有任何库的事实）；
 * 2. 迁移中：`migrating` / `committing` / `waiting_for_lock`，两个 `databaseKind` 各自的应用阶段；
 * 3. `ready`：单库 ready **不**等于应用 ready（应用 ready 由 Host 的服务面装配说了算）；
 * 4. `failed`：`errorCode` + `failedPhase` + 细节码（`sqliteCode` / `systemCode` / `migrationId`）。
 *
 * ## 夹具的来源（登记，不假装）
 *
 * 计划里写「复用 M2 已录制的 `startup/storageState` 帧」，但本 checkout 里**没有那份录制**
 * （`packages/shared/test/fixtures/agent-protocol/`、`packages/desktop/tests/agent-runtime-oracle/fixtures/`
 * 都没有 `startup/storageState` 帧）。所以 `test/fixtures/startup-storage/*.json` 是**按声明 schema 构造**的，
 * 并当场用 `modeStorageStartupStateSchema.parse` 验过——它证明「翻译对**声明的协议形状**成立」，
 * 不证明「对某台机器某次真实运行录下来的字节成立」。差额登记在 `docs/specs/tauri-renderer-host.md`。
 */
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";

import {
  databaseStartupStateSchema,
  modeStorageStartupStateSchema,
  type ModeStorageStartupState,
} from "@mode/shared";

import {
  StartupStateMachine,
  applicationPhaseForDatabaseKind,
} from "../src/host-standalone/startupMapping.js";

const here = dirname(fileURLToPath(import.meta.url));
const fixtureDir = resolve(here, "fixtures/startup-storage");

/** 读一个夹具帧，并**先按运行时 schema 验过**（夹具本身不合法就没有资格当输入）。 */
function loadFrame(name: string): ModeStorageStartupState {
  const raw: unknown = JSON.parse(readFileSync(join(fixtureDir, name), "utf8"));
  const parsed = modeStorageStartupStateSchema.safeParse(raw);
  assert.ok(
    parsed.success,
    `${name} 必须是合法的 startup/storageState 帧：${parsed.success ? "" : parsed.error.message}`,
  );
  return parsed.data;
}

/** 每个状态输出都必须过 `databaseStartupStateSchema`（`.strict()`：多一个字段就会被渲染层丢掉）。 */
function assertStateShape(state: unknown, what: string): void {
  const parsed = databaseStartupStateSchema.safeParse(state);
  assert.ok(
    parsed.success,
    `${what} 必须符合 databaseStartupStateSchema：${parsed.success ? "" : parsed.error.message}`,
  );
}

function newMachine(startupId = "startup-fixture"): StartupStateMachine {
  // 时钟注入：夹具断言要与挂钟无关。
  return new StartupStateMachine({
    startupId,
    attemptId: "attempt-fixture-1",
    now: () => 1_700_000_000_000,
  });
}

test("starting：状态机初值就是 starting，且字段全过 schema", () => {
  const machine = newMachine();
  const state = machine.snapshot();
  assertStateShape(state, "初值");
  assert.equal(state.phase, "starting");
  assert.equal(state.sequence, 0);
  assert.deepEqual(state.disk, [], "Task 2 不采磁盘摘要，但字段必须在（schema 要求）");
  assert.equal(state.startupId, "startup-fixture");
  assert.equal(state.attemptId, "attempt-fixture-1");
  assert.equal(state.errorCode, undefined, "没失败就不得出现 errorCode");
});

test("迁移中：库内阶段进 databasePhase，应用阶段由 databaseKind 决定（两个库各一条）", () => {
  const machine = newMachine();
  const states = [
    "01-tasks-index-checking.json",
    "02-tasks-index-waiting-for-lock.json",
    "03-tasks-index-migrating.json",
    "04-tasks-index-committing.json",
  ].map((name) => {
    const state = machine.applyStorageFrame(loadFrame(name));
    assertStateShape(state, name);
    return state;
  });

  // 应用阶段：tasks-index 一律是「准备任务索引」，库内阶段原样带过来。
  for (const [index, state] of states.entries()) {
    assert.equal(
      state.phase,
      "preparing_host_storage",
      `第 ${index} 个帧的应用阶段应是 preparing_host_storage`,
    );
  }
  assert.equal(states[0]!.databasePhase, "checking");
  assert.equal(states[1]!.databasePhase, "waiting_for_lock");
  assert.equal(states[2]!.databasePhase, "migrating");
  assert.equal(states[3]!.databasePhase, "committing");

  // 迁移事实原样带过去；正在迁移/提交时另填 currentMigration（渲染层用它显示进度）。
  assert.deepEqual(states[2]!.migration, {
    kind: "upgrade",
    executedCount: 3,
    committedCount: 2,
    lastAppliedMigrationId: "2026-08-01-add-task-archive",
  });
  assert.deepEqual(states[2]!.currentMigration, states[2]!.migration);
  assert.equal(states[0]!.currentMigration, undefined, "没在迁移时不得出现 currentMigration");

  // 库身份与迁移基线：`{databaseId, databaseKind, lastAppliedMigrationId}` 是渲染层重试判据的输入。
  assert.deepEqual(states[2]!.migrationBaselines, [
    {
      databaseId: "tasks-index",
      databaseKind: "tasks-index",
      lastAppliedMigrationId: "2026-08-01-add-task-archive",
    },
  ]);

  // `sequence` 是本机发布计数器（单调、从 0 开始），与帧里那个**单库流**序号不是一回事。
  assert.deepEqual(
    states.map((state) => state.sequence),
    [1, 2, 3, 4],
  );

  // 另一个 databaseKind：应用阶段不同，库内阶段同源。
  const session = machine.applyStorageFrame(loadFrame("06-session-migrating.json"));
  assertStateShape(session, "06-session-migrating.json");
  assert.equal(session.phase, "preparing_session_storage");
  assert.equal(session.databasePhase, "migrating");
  assert.deepEqual(session.migrationBaselines, [
    { databaseId: "session-3f0c1a", databaseKind: "session", lastAppliedMigrationId: null },
  ]);
});

test("ready：单个库 ready 不等于应用 ready（应用 ready 只在服务面装配完成后发布）", () => {
  const machine = newMachine();
  const tasksReady = machine.applyStorageFrame(loadFrame("05-tasks-index-ready.json"));
  assertStateShape(tasksReady, "05-tasks-index-ready.json");
  assert.equal(tasksReady.phase, "preparing_host_storage", "库 ready 时应用阶段仍停在准备中");
  assert.equal(tasksReady.databasePhase, "ready");

  const sessionReady = machine.applyStorageFrame(loadFrame("07-session-ready.json"));
  assertStateShape(sessionReady, "07-session-ready.json");
  assert.equal(sessionReady.phase, "preparing_session_storage");
  assert.equal(sessionReady.databasePhase, "ready");

  // 应用的 ready 由 Host 自己发布（真服务面装配完成之后）。
  const services = machine.report("starting_services");
  assertStateShape(services, "starting_services");
  assert.equal(services.phase, "starting_services");
  const ready = machine.report("ready");
  assertStateShape(ready, "ready");
  assert.equal(ready.phase, "ready");
  assert.equal(
    ready.sequence,
    services.sequence + 1,
    "sequence 单调递增：渲染层按它丢弃旧状态",
  );
});

test("failed：errorCode + failedPhase + 细节码都必须能落到渲染层读的字段上", () => {
  const machine = newMachine();
  const failedTasks = machine.applyStorageFrame(loadFrame("08-tasks-index-failed-permission.json"));
  assertStateShape(failedTasks, "08-tasks-index-failed-permission.json");
  assert.equal(failedTasks.phase, "failed");
  assert.equal(failedTasks.errorCode, "permission_denied");
  assert.equal(
    failedTasks.failedPhase,
    "preparing_host_storage",
    "失败要说出停在哪个**应用**阶段：那是渲染层的重试判据（canRetryDatabaseStartup）",
  );
  assert.equal(failedTasks.sqliteCode, 3, "细节码只带码，不带 SQL/文件内容");
  assert.equal(failedTasks.systemCode, "EACCES");

  const failedSession = newMachine().applyStorageFrame(
    loadFrame("09-session-failed-lock-timeout.json"),
  );
  assertStateShape(failedSession, "09-session-failed-lock-timeout.json");
  assert.equal(failedSession.errorCode, "lock_timeout");
  assert.equal(failedSession.failedPhase, "preparing_session_storage");
  assert.equal(failedSession.migrationId, "2026-08-01-add-task-archive");
});

test("failed（本机路径）：异常用仓库既有分类，不自己编错码；无分类依据时落 sql_failed", () => {
  const machine = newMachine();
  const noSpace = machine.fail(Object.assign(new Error("disk full"), { code: "ENOSPC" }));
  assertStateShape(noSpace, "fail(ENOSPC)");
  assert.equal(noSpace.phase, "failed");
  assert.equal(noSpace.errorCode, "storage_full");
  assert.equal(noSpace.failedPhase, "starting", "失败发生在起始阶段就如实记 starting");

  const unknown = newMachine().fail(new Error("something odd"));
  assertStateShape(unknown, "fail(unknown)");
  assert.equal(unknown.errorCode, "sql_failed", "分类不出更精确的码时落 sql_failed（既有缺省）");

  // 显式指定的错码优先（壳侧用 `unsupported_runtime` 之类的码时走这条）。
  const explicit = newMachine().fail(new Error("x"), { errorCode: "unsupported_runtime" });
  assert.equal(explicit.errorCode, "unsupported_runtime");
});

test("全部夹具都覆盖到位：每个 databaseKind × 关键库内阶段都能翻译，且没有未映射的 kind", () => {
  const names = readdirSync(fixtureDir).filter((name) => name.endsWith(".json"));
  assert.ok(names.length >= 6, `夹具数量应覆盖四条路径，实际 ${names.length}`);
  const machine = newMachine();
  const seen = new Set<string>();
  for (const name of names.sort()) {
    const frame = loadFrame(name);
    const state = machine.applyStorageFrame(frame);
    assertStateShape(state, name);
    seen.add(`${frame.databaseKind}:${frame.phase}`);
    // 应用阶段必须与 databaseKind 的映射一致（不硬凑同名）。
    if (frame.phase !== "failed") {
      assert.equal(state.phase, applicationPhaseForDatabaseKind(frame.databaseKind));
    }
  }
  for (const required of [
    "tasks-index:checking",
    "tasks-index:waiting_for_lock",
    "tasks-index:migrating",
    "tasks-index:committing",
    "tasks-index:ready",
    "tasks-index:failed",
    "session:migrating",
    "session:ready",
    "session:failed",
  ]) {
    assert.ok(seen.has(required), `夹具缺少 ${required}`);
  }
});

test("库内事实路径（= 生产路径）：帧带不了的相位（`maintaining`）也能翻译并过 schema", () => {
  // 这条守的是复评 I1 的核心：**出货的那条路**（真存储回调 → `applyStorageFact`）必须是被断言的
  // 那一个函数。顺带钉住一个真实的类型差：wire 帧的 phase 枚举里**没有** `maintaining`
  // （`modeStorageStartupStateSchema`），而真存储的 `TasksStoragePhase` 有——所以「事实」比「帧」
  // 宽一格，只有事实路径能表达它，帧路径永远测不到这一格。
  const machine = newMachine();
  const maintaining = machine.applyStorageFact({
    databaseId: "tasks-index",
    databaseKind: "tasks-index",
    phase: "maintaining",
  });
  assertStateShape(maintaining, "maintaining（事实路径）");
  assert.equal(maintaining.phase, "preparing_host_storage", "应用阶段由 databaseKind 决定");
  assert.equal(
    maintaining.databasePhase,
    "maintaining",
    "`maintaining` 必须能落到 databasePhase（渲染层的词汇表允许它）",
  );

  // 事实路径的失败形态与帧路径同源（同一个实现），所以失败字段的语义也一致。
  const failed = newMachine().applyStorageFact({
    databaseId: "session-1",
    databaseKind: "session",
    phase: "failed",
    errorCode: "lock_timeout",
    migrationId: "2026-08-01-add-task-archive",
  });
  assertStateShape(failed, "fact-failed");
  assert.equal(failed.phase, "failed");
  assert.equal(failed.failedPhase, "preparing_session_storage");
  assert.equal(failed.errorCode, "lock_timeout");
  assert.equal(failed.migrationId, "2026-08-01-add-task-archive");
});

test("生产入口不得自己写应用阶段：翻译只有一份实现，且入口走它（复评 I1 的守卫）", () => {
  // 复评 I1 的原始缺陷：入口把应用阶段硬编码成字面量，于是**夹具断言的是死代码**
  // （夹具测 `applyStorageFrame`，出货的是另一段内联映射）——绿的是没跑的那条路。
  // 这条源码扫描（本仓对跨语言字面量的同一套做法）钉住两件事：入口必须调 `applyStorageFact`，
  // 且**代码**里不得出现 storage 派生出来的那两个应用阶段字面量。
  // 注释要先剥掉：解释这个缺陷的注释本身就会引用那两个名字（写这条用例时真踩到过），
  // 所以扫描对象是剥掉块注释与整行注释之后的源码——只剥这两种，本文件的字符串里没有 `//`。
  const sourcePath = resolve(here, "../src/host-standalone/index.ts");
  const code = readFileSync(sourcePath, "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^[ \t]*\/\/.*$/gm, "");

  assert.ok(
    code.includes("applyStorageFact("),
    "入口必须调用 StartupStateMachine.applyStorageFact（与夹具同一条路径）",
  );
  for (const phase of ["preparing_host_storage", "preparing_session_storage"]) {
    assert.ok(
      !code.includes(`"${phase}"`),
      `入口不得硬编码应用阶段 "${phase}"：那是 applicationPhaseForDatabaseKind 的职责` +
        "（硬编码 = 夹具测的是死代码，出货的那条路无人断言）",
    );
  }
  assert.doesNotMatch(
    code,
    /startup\.report\(\s*"preparing_/,
    "入口不得直接 report storage 派生的应用阶段（必须经 applyStorageFact）",
  );
});
