// M2 Task 6 Step 0 探针：审批触发面、风险分级、拒绝与 allow_project 的匹配。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-permission.mts
//
// 纪律：只运行旧实现、观察请求与结果，不读实现体。
// 三个用例：工具/命令矩阵、deny 的答复形状、allow_project 后的规则匹配。
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";
import type { MockStep } from "./mock-provider.mjs";

type RawFrame = Record<string, unknown>;
type Observed = { type: string; payload: Record<string, unknown> };
type Captured = { frame: RawFrame; at: number };

function observed(frame: RawFrame): Observed | undefined {
  if (frame.method !== "session/event") return undefined;
  const params = (frame.params ?? {}) as Record<string, unknown>;
  const type = typeof params.type === "string" ? params.type : undefined;
  if (!type) return undefined;
  const payload =
    typeof params.payload === "object" && params.payload !== null
      ? (params.payload as Record<string, unknown>)
      : {};
  return { type, payload };
}

async function runCase(
  title: string,
  script: MockStep[],
  seed: Record<string, string>,
  drive: (context: {
    session: Awaited<ReturnType<typeof startOracleRuntime>>;
    captured: Captured[];
  }) => Promise<void>,
  permissionReply?: (params: Record<string, unknown>) => unknown,
): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-perm-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-perm-ws-"));
  for (const [path, content] of Object.entries(seed)) {
    await writeFile(join(cwd, path), content);
  }
  const captured: Captured[] = [];
  const session = await startOracleRuntime({
    dataRoot,
    cwd,
    script,
    permissionReply,
    onRawFrame: (frame) => captured.push({ frame, at: Date.now() }),
  });
  try {
    process.stdout.write(`\n########## ${title} ##########\n`);
    await drive({ session, captured });
  } finally {
    await session.stop();
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

function waitForTerminals(captured: readonly Captured[], count: number, timeoutMs: number) {
  return (async () => {
    const deadline = Date.now() + timeoutMs;
    while (true) {
      const terminals = captured.filter(({ frame }) => {
        const event = observed(frame);
        return event?.type === "turn.completed" || event?.type === "turn.failed";
      }).length;
      if (terminals >= count) return;
      if (Date.now() > deadline) throw new Error(`probe：${timeoutMs}ms 内只等到 ${terminals} 个终态`);
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  })();
}

function toolNameById(captured: readonly Captured[]): Map<string, string> {
  const names = new Map<string, string>();
  for (const { frame } of captured) {
    const event = observed(frame);
    if (event?.type !== "tool.updated" || event.payload.kind !== "scheduled") continue;
    names.set(String(event.payload.toolCallId), String(event.payload.toolName));
  }
  return names;
}

function printPermissionMatrix(captured: readonly Captured[]): void {
  const names = toolNameById(captured);
  const permissions = new Map<string, { riskLevel: unknown; reason: unknown; options: unknown }>();
  for (const { frame } of captured) {
    const event = observed(frame);
    if (event?.type !== "permission.requested") continue;
    const payload = event.payload;
    const options = (payload.options as { optionId?: string }[] | undefined)?.map(
      (option) => option.optionId,
    );
    permissions.set(String(payload.toolCallId), {
      riskLevel: payload.riskLevel,
      reason: payload.reason,
      options,
    });
  }
  const results = new Map<string, string>();
  for (const { frame } of captured) {
    const event = observed(frame);
    if (event?.type !== "tool.updated") continue;
    const payload = event.payload;
    if (payload.kind === "result") results.set(String(payload.toolCallId), "ok");
    if (payload.kind === "error") results.set(String(payload.toolCallId), "error");
  }
  for (const [toolCallId, name] of names) {
    const permission = permissions.get(toolCallId);
    process.stdout.write(
      `${toolCallId} ${name}: permission=${permission ? JSON.stringify(permission) : "none"} result=${results.get(toolCallId) ?? "unknown"}\n`,
    );
  }
}

const cwdPlaceholder = "${cwd}";
const testCases: MockStep[] = [
  { toolCalls: [{ name: "Read", input: { file_path: `${cwdPlaceholder}/hello.txt` } }] },
  {
    toolCalls: [
      { name: "Write", input: { file_path: `${cwdPlaceholder}/new.txt`, content: "hello\n" } },
    ],
  },
  {
    toolCalls: [
      {
        name: "Edit",
        input: {
          file_path: `${cwdPlaceholder}/new.txt`,
          old_string: "hello",
          new_string: "hi",
        },
      },
    ],
  },
  { toolCalls: [{ name: "Bash", input: { command: "echo hi", description: "Echo" } }] },
  {
    toolCalls: [
      { name: "Bash", input: { command: "echo a; echo b", description: "Sequence" } },
    ],
  },
  {
    toolCalls: [
      { name: "Bash", input: { command: "echo a && echo b", description: "And chain" } },
    ],
  },
  {
    toolCalls: [
      { name: "Bash", input: { command: "sleep 1 &", description: "Background ampersand" } },
    ],
  },
  {
    toolCalls: [
      { name: "Bash", input: { command: "rm -rf nonexistent-dir", description: "Remove dir" } },
    ],
  },
  { text: "permission matrix done" },
];

await runCase("审批矩阵（默认 allow）", testCases, { "hello.txt": "hello\n" }, async ({ session, captured }) => {
  await session.send("probe permission matrix");
  await waitForTerminals(captured, 1, 60_000);
  printPermissionMatrix(captured);
});

// deny 的答复形状。
await runCase(
  "deny 一个 Write",
  [
    {
      toolCalls: [
        { name: "Write", input: { file_path: "${cwd}/denied.txt", content: "no\n" } },
      ],
    },
    { text: "after deny" },
  ],
  {},
  async ({ session, captured }) => {
    await session.send("probe deny");
    await waitForTerminals(captured, 1, 30_000);
    for (const { frame } of captured) {
      const event = observed(frame);
      if (!event) continue;
      if (event.type === "tool.updated") {
        process.stdout.write(
          `tool.updated kind=${String(event.payload.kind)} ${JSON.stringify(event.payload)}\n`,
        );
      } else if (event.type === "permission.requested") {
        process.stdout.write(`permission requested: ${JSON.stringify(event.payload.input)}\n`);
      } else if (event.type === "turn.completed") {
        process.stdout.write(
          `turn.completed: resultType=${String(event.payload.resultType)} response=${JSON.stringify(event.payload.response)}\n`,
        );
      }
    }
  },
  () => ({
    decision: "deny",
    reason: "The user doesn't want to proceed with this tool use.",
  }),
);

// allow_project 之后的匹配：同路径不应再问，不同路径应再问。
let permissionCall = 0;
const requestedPaths: string[] = [];
await runCase(
  "allow_project 的匹配范围",
  [
    { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/a.txt", content: "a\n" } }] },
    { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/a.txt", content: "a2\n" } }] },
    { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/b.txt", content: "b\n" } }] },
    { text: "allow project done" },
  ],
  {},
  async ({ session, captured }) => {
    await session.send("probe allow_project");
    await waitForTerminals(captured, 1, 30_000);
    process.stdout.write(`permission requests: ${permissionCall}\n`);
    process.stdout.write(`requested paths: ${JSON.stringify(requestedPaths)}\n`);
    for (const { frame } of captured) {
      const event = observed(frame);
      if (event?.type === "tool.updated" && event.payload.kind === "result") {
        process.stdout.write(`result ok: ${String(event.payload.toolCallId)}\n`);
      }
      if (event?.type === "tool.updated" && event.payload.kind === "error") {
        process.stdout.write(`result error: ${JSON.stringify(event.payload.error)}\n`);
      }
    }
  },
  (params) => {
    permissionCall += 1;
    const input = (params.input ?? {}) as Record<string, unknown>;
    const path = String(input.file_path ?? "");
    requestedPaths.push(path);
    if (permissionCall === 1) {
      return {
        decision: "allow",
        reason: "Approved for this project",
        permissionUpdates: [
          {
            type: "addRules",
            behavior: "allow",
            rules: [{ toolName: "Write", ruleContent: path }],
          },
        ],
      };
    }
    return { decision: "allow", reason: "Approved once" };
  },
);
