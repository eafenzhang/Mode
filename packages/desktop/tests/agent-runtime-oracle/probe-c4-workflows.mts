/* oxlint-disable eslint(max-lines) -- 保存生命周期与运行观察共享同一套 Skill 门禁/文件投影，拆散会复制规则。 */
// C4 Task 0 观察项 4：Workflows 的黑盒观察。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-workflows.mts
//
// 触发方式：`dynamicWorkflowEnabled:true` 的会话里用运行时自己的工具面落盘
// （Skill(dynamic-workflows) 门禁 → SaveWorkflow），再走 `workflows` 六方法与
// `v4/command startSavedWorkflow`。合法文件形状（`.dwf.ts` + `/* mode-workflow` 块注释）
// 由 SaveWorkflow 的落盘产物逐字节记录；stdio 面对非法形状的拒绝也逐条记录。
// 观察项：六方法形状、move 双向、run 事件/actor/node/artifacts v4 查询、dwf_* DB 行。
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import {
  isolationRecord,
  startC4Harness,
  writeC4Fixture,
  type C4Harness,
} from "./probe-c4-support.mjs";

const SEED = { "hello.txt": "hello\n" };
const MINIMAL_SCRIPT = "return { ok: true };";

function lastUserText(body: JsonObject): string {
  const messages = Array.isArray(body.messages) ? (body.messages as JsonObject[]) : [];
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role !== "user") continue;
    if (typeof message.content === "string") return message.content;
    if (Array.isArray(message.content)) {
      return (message.content as JsonObject[])
        .map((part) => (typeof part.text === "string" ? part.text : ""))
        .join("\n");
    }
  }
  return "";
}

/** 触发词 → 工具调用的状态机（Skill 门禁 → CreateWorkflow/SaveWorkflow）。 */
function gateScript(
  triggers: { needle: string; call: { name: string; input: JsonObject } }[],
  doneText: string,
): (body: JsonObject) => JsonObject {
  const consumed = new Set<string>();
  return (body) => {
    const text = lastUserText(body);
    for (const trigger of triggers) {
      if (text.includes(trigger.needle) && !consumed.has(trigger.needle)) {
        consumed.add(trigger.needle);
        return { toolCalls: [trigger.call] };
      }
    }
    if (consumed.size > 0 && consumed.size <= triggers.length) return { text: doneText };
    return { text: "workflow answer" };
  };
}

/** v4 startSavedWorkflow 命令的调用 + 规范化。 */
async function startSavedWorkflow(
  harness: C4Harness,
  name: string,
  scope: "project" | "global",
  commandId: string,
): Promise<{ ok: boolean; payload: JsonObject }> {
  const result = await harness.tryRequest("v4/command", {
    commandId,
    clientId: "probe-c4",
    sessionId: harness.session.sessionId,
    type: "startSavedWorkflow",
    payload: { name, scope },
    issuedAt: Date.now(),
  });
  return {
    ok: result.ok,
    payload: (result.ok ? result.result : result.error) as JsonObject,
  };
}

/** Case 1：Skill 门禁 + CreateWorkflow 未加载 skill 的拒绝 + SaveWorkflow 落盘 + 六方法。 */
async function caseSavedLifecycle(): Promise<JsonObject> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c4-wf-life-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c4-wf-life-ws-"));
  const harness = await startC4Harness({
    prefix: "c4-wf-life",
    seed: SEED,
    dataRoot,
    cwd,
    script: [
      gateScript(
        [
          {
            needle: "create without skill",
            call: {
              name: "CreateWorkflow",
              input: { name: "probe", script: MINIMAL_SCRIPT },
            },
          },
          {
            needle: "load the skill",
            call: { name: "Skill", input: { skill: "dynamic-workflows" } },
          },
          {
            needle: "save the workflow",
            call: {
              name: "SaveWorkflow",
              input: {
                name: "probe-wf",
                description: "probe workflow saved by tool",
                whenToUse: "use when the C4 probe runs the saved workflow",
                script: MINIMAL_SCRIPT,
                scope: "project",
              },
            },
          },
          {
            needle: "save global workflow",
            call: {
              name: "SaveWorkflow",
              input: {
                name: "global-probe",
                description: "probe global workflow",
                script: MINIMAL_SCRIPT,
                scope: "global",
              },
            },
          },
        ],
        "workflow turn done",
      ),
    ],
    sessionCreateParams: { dynamicWorkflowEnabled: true },
  });
  try {
    const turnStart = harness.eventCount();
    await harness.session.send("create without skill please");
    await harness.waitForEvent(
      turnStart,
      (event) => event.type === "turn.completed",
      "refusal turn",
      60_000,
    );
    const refusalEvents = harness.eventsSince(turnStart);
    // 拒绝是 kind:"result" 但载荷带 error 字段（工具执行失败），两个键都投影。
    const refusalResult = refusalEvents
      .filter(
        (event) => event.type === "tool.updated" && (event.payload as JsonObject).kind === "result",
      )
      .map((event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return payload.result ?? payload.error ?? null;
      })
      .at(-1);
    const refusalToolEvents = refusalEvents
      .filter((event) => event.type === "tool.updated")
      .map((event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return {
          kind: payload.kind ?? null,
          toolName: payload.toolName ?? null,
          errorType: ((payload.error as JsonObject | undefined) ?? {}).type ?? null,
          errorMessage: ((payload.error as JsonObject | undefined) ?? {}).message ?? null,
          success: ((payload.result as JsonObject | undefined) ?? {}).success ?? null,
        };
      });

    // 三次 send 各触发一个状态（触发词按 last user 消息匹配，历史会污染 includes，
    // 所以每轮一个 needle，且 Skill 与 SaveWorkflow 不能在同一条 prompt 里）。
    await harness.session.send("load the skill please");
    await harness.waitForEvent(
      harness.eventCount(),
      (event) => event.type === "turn.completed",
      "skill load turn",
      60_000,
    );
    await harness.session.send("save the workflow now");
    await harness.waitForEvent(
      harness.eventCount(),
      (event) => event.type === "turn.completed",
      "save turn",
      60_000,
    );
    await harness.session.send("save global workflow too");
    await harness.waitForEvent(
      harness.eventCount(),
      (event) => event.type === "turn.completed",
      "global save turn",
      60_000,
    );

    const savedFile = join(cwd, ".mode", "workflows", "probe-wf.dwf.ts");
    let savedBytes: string | null = null;
    try {
      savedBytes = await readFile(savedFile, "utf8");
    } catch {
      savedBytes = null;
    }

    const list = await harness.tryRequest("workflows/list", { workspace: harness.workspaceRef() });
    const get = await harness.tryRequest("workflows/get", {
      workspace: harness.workspaceRef(),
      name: "probe-wf",
    });
    const globalList = await harness.tryRequest("workflows/list", {
      workspace: harness.workspaceRef(),
      scope: "global",
    });
    const updateMeta = await harness.tryRequest("workflows/updateMeta", {
      workspace: harness.workspaceRef(),
      name: "probe-wf",
      meta: {
        description: "probe workflow saved by tool (meta updated)",
        whenToUse: "updated whenToUse",
        args: {
          target: { type: "string", description: "probe arg", required: false, default: "x" },
        },
      },
    });
    const getAfterMeta = await harness.tryRequest("workflows/get", {
      workspace: harness.workspaceRef(),
      name: "probe-wf",
    });
    const move = await harness.tryRequest("workflows/move", {
      workspace: harness.workspaceRef(),
      name: "global-probe",
    });
    const listAfterMove = await harness.tryRequest("workflows/list", {
      workspace: harness.workspaceRef(),
    });
    const deleteResult = await harness.tryRequest("workflows/delete", {
      workspace: harness.workspaceRef(),
      name: "global-probe",
    });
    const deleteMissing = await harness.tryRequest("workflows/delete", {
      workspace: harness.workspaceRef(),
      name: "missing-wf",
    });
    const getInvalidName = await harness.tryRequest("workflows/get", {
      workspace: harness.workspaceRef(),
      name: "BAD NAME!",
    });
    return {
      isolation: isolationRecord(harness),
      refusalResult,
      refusalToolEvents,
      savedBytes,
      list,
      get,
      globalList,
      updateMeta,
      getAfterMeta,
      move,
      listAfterMove,
      deleteResult,
      deleteMissing,
      getInvalidName,
    };
  } finally {
    await harness.stop();
  }
}

/** Case 2：非法文件（手写坏 frontmatter / 非 dwf 扩展名）在 list 的 invalid 形状。 */
async function caseInvalidFiles(): Promise<JsonObject> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c4-wf-invalid-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c4-wf-invalid-ws-"));
  const wfDir = join(cwd, ".mode", "workflows");
  const { mkdir } = await import("node:fs/promises");
  await mkdir(wfDir, { recursive: true });
  await writeFile(
    join(wfDir, "bad-frontmatter.dwf.ts"),
    "no frontmatter at all\nreturn 1;\n",
    "utf8",
  );
  await writeFile(
    join(wfDir, "missing-description.dwf.ts"),
    "---\nwhenToUse: no description key\n---\nreturn 2;\n",
    "utf8",
  );
  await writeFile(
    join(wfDir, "wrong-ext.ts"),
    "---\ndescription: wrong extension\n---\nreturn 3;\n",
    "utf8",
  );
  const harness = await startC4Harness({
    prefix: "c4-wf-invalid",
    seed: SEED,
    dataRoot,
    cwd,
    script: [{ text: "invalid workflows answer" }],
  });
  try {
    const list = await harness.tryRequest("workflows/list", { workspace: harness.workspaceRef() });
    const getBad = await harness.tryRequest("workflows/get", {
      workspace: harness.workspaceRef(),
      name: "bad-frontmatter",
    });
    return {
      isolation: isolationRecord(harness),
      list,
      getBad,
    };
  } finally {
    await harness.stop();
  }
}

/** Case 3：startSavedWorkflow 运行 → run 事件/actor/node/artifacts 查询 + dwf_* DB 行。 */
async function caseRun(): Promise<JsonObject> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c4-wf-run-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c4-wf-run-ws-"));
  const harness = await startC4Harness({
    prefix: "c4-wf-run",
    seed: SEED,
    dataRoot,
    cwd,
    script: [
      gateScript(
        [
          {
            needle: "load the skill",
            call: { name: "Skill", input: { skill: "dynamic-workflows" } },
          },
          {
            needle: "save the workflow",
            call: {
              name: "SaveWorkflow",
              input: {
                name: "probe-run",
                description: "probe workflow for run observation",
                script: MINIMAL_SCRIPT,
                scope: "project",
              },
            },
          },
        ],
        "run setup done",
      ),
    ],
    sessionCreateParams: { dynamicWorkflowEnabled: true },
  });
  try {
    await harness.session.send("load the skill please");
    await harness.waitForEvent(
      harness.eventCount(),
      (event) => event.type === "turn.completed",
      "skill load turn",
      120_000,
    );
    await harness.session.send("save the workflow now");
    await harness.waitForEvent(
      harness.eventCount(),
      (event) => event.type === "turn.completed",
      "save turn",
      120_000,
    );
    const accepted = await startSavedWorkflow(harness, "probe-run", "project", "cmd-c4-wf-run-ok");
    const rejected = await startSavedWorkflow(
      harness,
      "missing-wf",
      "project",
      "cmd-c4-wf-run-missing",
    );
    const runId = String(
      ((accepted.payload.result as JsonObject | undefined)?.runId as string | undefined) ?? "",
    );
    // extras 的键是「要折叠的原始值」，值是替换后的稳定 token。
    if (runId.length > 0) harness.extras[runId] = "<run-id>";
    // 等运行终态：workflows/runs 的 status 变为 completed/errored/stopped。
    const deadline = Date.now() + 60_000;
    let runs: JsonObject = {};
    while (Date.now() < deadline) {
      const page = await harness.tryRequest("workflows/runs", {
        workspace: harness.workspaceRef(),
        limit: 10,
      });
      runs = (page.ok ? page.result : { error: page.error }) as JsonObject;
      const items = (runs.runs ?? []) as JsonObject[];
      if (
        items.length > 0 &&
        items.every((run) => ["completed", "errored", "stopped"].includes(String(run.status)))
      ) {
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
    const v4runs = await harness.tryRequest("v4/conversation/workflowRuns", {
      sessionId: harness.session.sessionId,
    });
    const runEvents = runId
      ? await harness.tryRequest("v4/conversation/workflowRunEvents", {
          sessionId: harness.session.sessionId,
          runId,
        })
      : { ok: false, error: { note: "no runId" } as JsonObject };
    const runArtifacts = runId
      ? await harness.tryRequest("v4/conversation/workflowRunArtifacts", {
          sessionId: harness.session.sessionId,
          runId,
        })
      : null;
    const runWorkspace = runId
      ? await harness.tryRequest("v4/conversation/workflowRunWorkspace", {
          sessionId: harness.session.sessionId,
          runId,
        })
      : null;
    const runNodeResult = runId
      ? await harness.tryRequest("v4/conversation/workflowRunNodeResult", {
          sessionId: harness.session.sessionId,
          runId,
          nodeId: "probe-node",
        })
      : null;
    const runsAfter = await harness.tryRequest("workflows/runs", {
      workspace: harness.workspaceRef(),
      name: "probe-run",
      limit: 5,
    });
    const tables = harness.readC4Tables([
      "dwf_run",
      "dwf_event",
      "dwf_node",
      "dwf_actor",
      "workflow_definition",
      "workflow_run",
      "workflow_event",
    ]);
    return {
      isolation: isolationRecord(harness),
      accepted,
      rejected,
      runs,
      v4runs,
      runEvents,
      runArtifacts,
      runWorkspace,
      runNodeResult,
      runsByName: runsAfter,
      dwfTables: tables,
    };
  } finally {
    await harness.stop();
  }
}

const fixtures: Record<string, JsonObject> = {
  "workflows-saved-lifecycle": {
    case: "workflows-saved-lifecycle",
    note:
      "C4 Task 0 oracle 捕获：dynamicWorkflowEnabled 会话的 Skill(dynamic-workflows) 门禁、" +
      "CreateWorkflow 未加载 skill 的拒绝文案、SaveWorkflow(project/global) 落盘（.dwf.ts + " +
      "frontmatter 逐字节）、workflows 六方法形状（list/get/updateMeta/delete/move + 边界）。",
    input: {
      seed: SEED,
      dynamicWorkflowEnabled: true,
      savedScript: MINIMAL_SCRIPT,
      prompts: [
        "create without skill",
        "load the skill then save the workflow",
        "save global workflow too",
      ],
    },
    expect: await caseSavedLifecycle(),
  },
  "workflows-invalid-files": {
    case: "workflows-invalid-files",
    note:
      "C4 Task 0 oracle 捕获：手写非法工作流文件（无 frontmatter / 缺 description / 非 .dwf.ts " +
      "扩展名）在 workflows/list 的 workflows/invalid 归类与 workflows/get 的失败形状。",
    input: {
      files: [
        "bad-frontmatter.dwf.ts (no frontmatter)",
        "missing-description.dwf.ts",
        "wrong-ext.ts",
      ],
    },
    expect: await caseInvalidFiles(),
  },
  "workflows-run": {
    case: "workflows-run",
    note:
      "C4 Task 0 oracle 捕获：v4/command startSavedWorkflow 的 accepted ACK（runId dwfrun-*、" +
      "toolCallId launch-*）与 not_found 拒绝（fault.command.savedWorkflowStartRejected.not_found）、" +
      "workflows/runs 与 v4/conversation/workflowRuns 的 run 行、workflowRunEvents/Artifacts/" +
      "Workspace/NodeResult 查询形状、dwf_* DB 行。",
    input: { seed: SEED, dynamicWorkflowEnabled: true, savedScript: MINIMAL_SCRIPT },
    expect: await caseRun(),
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
          const list = (expect.list as JsonObject | undefined)?.result as JsonObject | undefined;
          return [
            name,
            {
              workflows: Array.isArray(list?.workflows)
                ? (list.workflows as JsonObject[]).map((entry) => entry.name)
                : null,
              invalid: Array.isArray(list?.invalid) ? list.invalid.length : null,
              runStatus:
                ((expect.runs as JsonObject | undefined)?.runs as JsonObject[] | undefined)?.[0]
                  ?.status ?? null,
            },
          ];
        }),
      ),
    },
    null,
    2,
  )}\n`,
);
