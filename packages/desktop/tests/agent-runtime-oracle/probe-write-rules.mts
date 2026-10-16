// M2 Task 3 探针：审批**规则的累积与作用域**（诊断用，不进对拍套件）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-write-rules.mts
//
// 要回答的问题（Task 3 Step 1 的收口）：
//   1. 多条 `allow_project` 是**累积**进同一个 ruleset，还是各写一行？
//   2. `deny` 会不会也落盘？
//   3. `scope_id` 的 slug 是不是「cwd 小写 + 非字母数字折成 `-`」？
//   4. 两个不同的 cwd 会不会互相污染？
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";
import { drain } from "./probe-support.mjs";
import { dumpLocalSetting, optionResponse, withIsolatedRoots } from "./probe-write-support.mjs";

/** 一次会话、三连写：两条 allow_project（不同路径）+ 一条 deny。 */
async function runMixedSession(dataRoot: string, cwd: string): Promise<void> {
  const requests: Record<string, unknown>[] = [];
  const decisions = ["allow_project", "allow_project", "deny"];
  let index = 0;
  const session = await startOracleRuntime({
    dataRoot,
    cwd,
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/a.txt" } }] },
      { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/a.txt", content: "A\n" } }] },
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/b.txt" } }] },
      { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/b.txt", content: "B\n" } }] },
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/c.txt" } }] },
      { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/c.txt", content: "C\n" } }] },
      { text: "mixed done" },
    ],
    onRawFrame: (frame) => {
      if (frame.method === "interaction/requestPermission") {
        requests.push(frame.params as Record<string, unknown>);
      }
    },
    permissionReply: (params) => {
      const decision = decisions[index] ?? "allow_once";
      index += 1;
      return optionResponse(params, decision);
    },
  });
  try {
    await session.send("mixed writes");
    await drain(session.events);
    process.stdout.write(
      `\n== 审批请求 ${requests.length} 次；回报序列 ${decisions.join(",")} ==\n` +
        `  请求里的 toolName/ruleContent：${JSON.stringify(
          requests.map((request) => {
            const input = (request.input ?? {}) as Record<string, unknown>;
            return { toolName: request.toolName, file_path: input.file_path };
          }),
        )}\n`,
    );
    for (const relative of ["a.txt", "b.txt", "c.txt"]) {
      const outcome = await readMaybe(join(cwd, relative));
      process.stdout.write(`  ${relative}: ${outcome}\n`);
    }
  } finally {
    await session.stop();
  }
}

async function readMaybe(path: string): Promise<string> {
  const { readFile } = await import("node:fs/promises");
  return readFile(path, "utf8").then(
    (text) => JSON.stringify(text),
    () => "（不存在）",
  );
}

await withIsolatedRoots(async ({ dataRoot, cwd }) => {
  process.stdout.write(`\n########## cwd = ${cwd} ##########\n`);
  await runMixedSession(dataRoot, cwd);
  process.stdout.write("\n### local_setting ###\n");
  await dumpLocalSetting(dataRoot);

  // 第二个 cwd：验证 scope_id 的 slug 形状与「两个项目互不污染」。
  const otherCwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-other-"));
  try {
    process.stdout.write(`\n########## 第二个 cwd = ${otherCwd} ##########\n`);
    await dumpLocalSetting(dataRoot); // 同一 dataRoot：第二个项目的行是否会先出现？不应。
    const otherSession = await startOracleRuntime({
      dataRoot,
      cwd: otherCwd,
      script: [
        { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/d.txt" } }] },
        { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/d.txt", content: "D\n" } }] },
        { text: "other done" },
      ],
      permissionReply: (params) => optionResponse(params, "allow_project"),
    });
    try {
      await otherSession.send("other write");
      await drain(otherSession.events);
    } finally {
      await otherSession.stop();
    }
    process.stdout.write("\n### 第二个 cwd 之后的 local_setting ###\n");
    await dumpLocalSetting(dataRoot);
  } finally {
    await rm(otherCwd, { recursive: true, force: true });
  }
}, { "a.txt": "a0\n", "b.txt": "b0\n", "c.txt": "c0\n" });
