/* oxlint-disable eslint(max-lines) -- automation 与 provider/account 两组调用共享同一套 tryRequest 投影。 */
// C4 Task 0 观察项 7：automation / provider / account 在 stdio 面的真实形状。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-automation.mts
//
// 背景：`host-ownership-switch.md` 决策 4 已把 automation 10 个 Host 聚合方法改判
// `not-needed`（Renderer 分调两个 owner），决策 2 让 provider/account 两个网络方法
// 返回显式 `unsupported_method`。本探针不改裁决，只回答「stdio agent 面到底可不可达、
// 不可达时的错误形状是什么」，供 C4 T5 引用。
//
// 观察项：
//   1. automation 协议镜像的 5 个方法（create/update/checkTaskBinding/list/delete）逐个调用；
//   2. 调用后会话是否仍可正常跑一个普通 turn；
//   3. provider/testModelConnectivity 与 provider/updateAccountConfig 的应答/错误；
//   4. 所有调用都不写真实凭据、不依赖外部网络（testModelConnectivity 指向隔离根里的
//      oracle-mock 配置；若运行时先做账号门禁则记录门禁错误）。
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import { eventSummary } from "./probe-c3-support.mjs";
import {
  isolationRecord,
  startC4Harness,
  writeC4Fixture,
  type C4TryResult,
} from "./probe-c4-support.mjs";

const SEED = { "hello.txt": "hello\n" };

/** 把 tryRequest 结果压成「ok + 结果」/「error + 错误」，夹具里不再区分外层结构。 */
function compact(result: C4TryResult): JsonObject {
  return result.ok ? { ok: true, result: result.result } : { ok: false, error: result.error };
}

/** Case 1：automation 5 个方法 + 会话可用性。 */
async function caseAutomation(): Promise<JsonObject> {
  const harness = await startC4Harness({
    prefix: "c4-automation",
    seed: SEED,
    script: [{ text: "plain answer after automation probe" }],
  });
  try {
    const sessionId = harness.session.sessionId;
    const calls = {
      create: compact(
        await harness.tryRequest("automation/create", {
          title: "probe automation",
          cronExpr: "0 9 * * *",
          prompt: "probe automation prompt",
          targetTaskId: sessionId,
        }),
      ),
      update: compact(
        await harness.tryRequest("automation/update", {
          automationId: "auto_probe_missing",
          prompt: "updated probe automation prompt",
        }),
      ),
      checkTaskBinding: compact(
        await harness.tryRequest("automation/checkTaskBinding", { targetTaskId: sessionId }),
      ),
      list: compact(await harness.tryRequest("automation/list", {})),
      delete: compact(
        await harness.tryRequest("automation/delete", { automationId: "auto_probe_missing" }),
      ),
    };
    const start = harness.eventCount();
    await harness.sendAndWait("plain prompt after automation calls");
    const events = harness.eventsSince(start);
    return {
      isolation: isolationRecord(harness),
      sessionId: "<session-id>",
      calls,
      sessionUsable: {
        eventSummary: eventSummary(events),
        turnCompleted: events.some((event) => event.type === "turn.completed"),
      },
    };
  } finally {
    await harness.stop();
  }
}

/** Case 2：provider 两个网络方法 + 会话可用性（testModelConnectivity 指 oracle-mock）。 */
async function caseProvider(): Promise<JsonObject> {
  const harness = await startC4Harness({
    prefix: "c4-provider",
    seed: SEED,
    script: [{ text: "plain answer after provider probe" }],
  });
  try {
    const testConnectivity = compact(
      await harness.tryRequest("provider/testModelConnectivity", {
        workspace: harness.workspaceRef(),
        selection: { providerId: "oracle-mock", modelId: "mock-1" },
      }),
    );
    const start = harness.eventCount();
    await harness.sendAndWait("plain prompt after connectivity probe");
    const events = harness.eventsSince(start);
    // 账户配置放在最后：空 providers/states 会替换进程级 Account Provider Config，
    // 之后不再发模型请求，避免把探针自身的副作用当成契约。
    const updateAccountConfig = compact(
      await harness.tryRequest("provider/updateAccountConfig", {
        revision: "probe-account-revision-1",
        basedOnModeBuiltinRevision: "probe-builtin-revision-1",
        providers: {},
        states: {},
      }),
    );
    return {
      isolation: isolationRecord(harness),
      testConnectivity,
      sessionUsable: {
        eventSummary: eventSummary(events),
        turnCompleted: events.some((event) => event.type === "turn.completed"),
      },
      updateAccountConfig,
    };
  } finally {
    await harness.stop();
  }
}

const fixtures: Record<string, JsonObject> = {
  "automation-stdio-surface": {
    case: "automation-stdio-surface",
    note:
      "C4 Task 0 oracle 捕获：automation 协议镜像的 5 个方法在 stdio agent 面的应答/错误形状，" +
      "以及调用后普通 turn 仍可完成；不改写决策 4 的 not-needed 裁决。",
    input: {
      seed: SEED,
      calls: [
        "automation/create {title,cronExpr,prompt,targetTaskId}",
        "automation/update {automationId:missing,prompt}",
        "automation/checkTaskBinding {targetTaskId:session}",
        "automation/list {}",
        "automation/delete {automationId:missing}",
      ],
      after: "plain prompt",
    },
    expect: await caseAutomation(),
  },
  "provider-account-stdio-surface": {
    case: "provider-account-stdio-surface",
    note:
      "C4 Task 0 oracle 捕获：provider/testModelConnectivity（selection 指向隔离根里的 " +
      "oracle-mock）与 provider/updateAccountConfig（空 providers/states，最后调用）的应答/错误形状；" +
      "不含真实凭据与外部网络。",
    input: {
      seed: SEED,
      selection: { providerId: "oracle-mock", modelId: "mock-1" },
      updateAccountConfig: { providers: {}, states: {} },
    },
    expect: await caseProvider(),
  },
};

const targets: string[] = [];
for (const [name, fixture] of Object.entries(fixtures)) {
  targets.push(await writeC4Fixture(name, fixture));
}
process.stdout.write(
  `${JSON.stringify(
    {
      fixtures: targets,
      summaries: Object.fromEntries(
        Object.entries(fixtures).map(([name, fixture]) => {
          const expect = fixture.expect as JsonObject;
          return [
            name,
            {
              calls: expect.calls ?? null,
              testConnectivity: expect.testConnectivity ?? null,
              updateAccountConfig: expect.updateAccountConfig ?? null,
              turnCompleted:
                (expect.sessionUsable as JsonObject | undefined)?.turnCompleted ?? null,
            },
          ];
        }),
      ),
    },
    null,
    2,
  )}\n`,
);
