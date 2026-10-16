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

  // 累积的边界：**同一条规则再来一次**会不会去重？「一次勾选带几条规则」？
  //
  // **必须换一个全新的隔离根**：同一个 dataRoot+cwd 下 a.txt/b.txt 已经被上面的会话允许过，
  // 规则生效后审批请求根本不会发（实测 `重复写：审批请求 0 次`），那样等于什么都没探到。
  process.stdout.write(`\n########## 累积边界（全新隔离根）##########\n`);
  await withIsolatedRoots(async (fresh) => {
    await runDuplicateSession(fresh.dataRoot, fresh.cwd);
    process.stdout.write("\n### 重复之后 ###\n");
    await dumpLocalSetting(fresh.dataRoot);
  }, { "a.txt": "a0\n", "b.txt": "b0\n" });

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

/**
 * 三次 `allow_project`：a.txt → b.txt → **a.txt 再来一次**（同 toolName+ruleContent）。
 * 观察 allow[] 是「追加」还是「去重」，以及一次更新里带了几条规则。
 */
async function runDuplicateSession(dataRoot: string, cwd: string): Promise<void> {
  const targets = ["a.txt", "b.txt", "a.txt"];
  const requests: Record<string, unknown>[] = [];
  const session = await startOracleRuntime({
    dataRoot,
    cwd,
    script: [
      ...targets.flatMap((name) => [
        { toolCalls: [{ name: "Read", input: { file_path: `\${cwd}/${name}` } }] },
        { toolCalls: [{ name: "Write", input: { file_path: `\${cwd}/${name}`, content: "z\n" } }] },
      ]),
      { text: "dup done" },
    ],
    onRawFrame: (frame) => {
      if (frame.method === "interaction/requestPermission") {
        requests.push(frame.params as Record<string, unknown>);
      }
    },
    permissionReply: (params) => optionResponse(params, "allow_project"),
  });
  try {
    await session.send("dup writes");
    await drain(session.events);
    process.stdout.write(
      `\n== 重复写：审批请求 ${requests.length} 次（第 3 次是 a.txt 的重放）==\n`,
    );
    for (const [index, request] of requests.entries()) {
      const options = Array.isArray(request.options) ? request.options : [];
      const project = options.find(
        (raw) => (raw as { optionId?: unknown }).optionId === "allow_project",
      ) as { response?: { permissionUpdates?: unknown[] } } | undefined;
      process.stdout.write(
        `  #${index} rules: ${JSON.stringify(project?.response?.permissionUpdates)}\n`,
      );
    }
  } finally {
    await session.stop();
  }
}
