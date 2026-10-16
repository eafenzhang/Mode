// M2 Task 2 探针：**运行时对外声明了哪些工具**（诊断用，不进对拍套件）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-tool-surface.mts [tools|absence]
//   MODE_PROBE_CLI_ARGS="--surface desktop --mode build" … tools      # 观察别的 CLI 参数下的清单
//
// ## 结论（2026-10-16 实测，原始输出见运行日志）
//
// 1. **清单固定 21 条，只读切片里只有 `Read` 注册**：`Glob` / `Grep` **不在里面**，而且脚本化调用
//    它们得到 `{"type":"tool_not_found","message":"Tool not found: Glob","code":"TOOL_NOT_FOUND"}`——
//    即**没有注册**（不是「注册了但看不见」）。见 `absence` 组。
//    这与计划里「只读切片 = Read/Glob/Grep」的写法冲突（那个写法是从 `MODE_KNOWN_TOOL_NAMES`
//    这张**渲染用名字表**推出来的，不是从运行时注册表推出来的）。
// 2. **清单不随 CLI 参数变化**：`--surface desktop`、`--mode build|plan`、`--disallowed-tools <工具>`
//    都不改变它。正对照最关键：`--disallowed-tools Bash` 下 **`Bash` 照样被声明**——如果这些参数
//    真的影响会话面工具表，`Bash` 就该消失。这条否证由 oracle 套件的
//    `tool-surface.test.mts` 自动跑（复现见那里的命令），`MODE_PROBE_CLI_ARGS` 是手工复现的入口。
// 3. `nativeSearchEnhancementsEnabled` 也不改变它（该偏好的语义由 `packages/shared/src/protocol.ts`
//    的字段注释给出：「为 Bash 注入 bfs/ugrep 增强」，改的是 Bash 的注入，不是工具注册表）。
// 4. 整个会话只有 **2 条**反向请求（`session/requestRuntimePreferences` 的
//    `runtime-materialization` 与 `user-execution` 各一次）——没有第三个偏好可以开关工具面。
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";
import { runtimePreferencesReply } from "./probe-support.mjs";

/**
 * **不存在**的工具（`Glob` / `Grep`）被脚本化调用时的结果——用来证明「会话面没有注册它们」，
 * 而不是「注册了但没对外声明」。见文件头结论 1。
 *
 * `Agent` 作为正对照放在同一组里：它**有**注册，所以拿到的是别的错误
 * （例如入参校验失败 / 子代理缺席），而不是 `tool_not_found`。
 */
function absenceGroup(): Group {
  const seed: Record<string, string> = {
    "one.txt": "alpha\nBeta\ngamma alpha\ndelta\n",
    "sub/two.ts": "const a = 1;\n",
  };
  const absent: Probe[] = [
    { label: "glob-star-txt", seed, args: { pattern: "*.txt" } },
    { label: "glob-no-match", seed, args: { pattern: "**/*.zzz" } },
    { label: "grep-content", tool: "Grep", seed, args: { pattern: "alpha", path: "${cwd}" } },
    {
      label: "grep-files-with-matches",
      tool: "Grep",
      seed,
      args: { pattern: "alpha", path: "${cwd}", output_mode: "files_with_matches" },
    },
    {
      label: "grep-count",
      tool: "Grep",
      seed,
      args: { pattern: "alpha", path: "${cwd}", output_mode: "count" },
    },
    { label: "grep-no-match", tool: "Grep", seed, args: { pattern: "zzzzz", path: "${cwd}" } },
    // 正对照：`Agent` **有**注册。故意给一个空入参，让它在**解析阶段**就失败
    // （`Tool input failed inputSchema validation`），这样不会真的去起子代理
    // （剧本索引按「assistant 消息数」取，子代理会递归拿到 `script[0]`，有起进程风暴的风险）。
    // 拿到「入参校验失败」而不是「Tool not found: Agent」就足以证明它注册了。
    { label: "agent-positive-control", tool: "Agent", seed, args: {} },
  ];
  return { tool: "Glob", name: "absence", probes: absent };
}

/** 追加的 CLI 参数（只为手工观察工具面差异；`MODE_PROBE_CLI_ARGS` 空格分隔）。 */
function extraCliArgs(): string[] {
  return (process.env.MODE_PROBE_CLI_ARGS ?? "").trim().split(/\s+/).filter(Boolean);
}

/** `tools` 组：把运行时**对外声明**的工具清单一字不差地打出来，并列出全部反向请求。 */
async function dumpToolDeclarations(): Promise<number> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  const cliArgs = extraCliArgs();
  const declarations: unknown[] = [];
  const nameLists: string[][] = [];
  const reverseRequests: string[] = [];
  const wanted = new Set(["Read", "Glob", "Grep"]);
  try {
    const session = await startOracleRuntime({
      dataRoot,
      cwd,
      script: [
        { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/x.txt" } }] },
        { text: "ok" },
      ],
      runtimePreferencesReply: runtimePreferencesReply(),
      extraCliArgs: cliArgs,
      onRawFrame: (frame) => {
        if (typeof frame.method === "string" && frame.id !== undefined) {
          reverseRequests.push(JSON.stringify(frame));
        }
      },
      onMockRequestBody: (_path, body) => {
        const tools = (body as { tools?: unknown[] }).tools;
        if (!Array.isArray(tools)) return;
        const names = tools.map((tool) => String((tool as { name?: string }).name ?? ""));
        nameLists.push(names);
        for (const tool of tools) {
          if (wanted.has((tool as { name?: string }).name ?? "")) declarations.push(tool);
        }
      },
    });
    try {
      await session.send("probe");
      await drain(session.events);
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
  process.stdout.write(
    `\n##### 反向请求（host 必须应答的那一半，${reverseRequests.length} 条）#####\n`,
  );
  for (const frame of reverseRequests) process.stdout.write(`${frame}\n`);
  for (const [index, names] of nameLists.entries()) {
    process.stdout.write(
      `\n##### 请求 #${index} 的工具清单（${names.length} 条）#####\n${names.join(",")}\n`,
    );
  }
  for (const declaration of declarations) {
    process.stdout.write(`\n--- tool declaration ---\n${JSON.stringify(declaration, null, 2)}\n`);
  }
  return declarations.length;
}

const requested = process.argv.slice(2);
const names = requested.length > 0 ? requested : ["tools", "absence"];
for (const name of names) {
  if (name === "tools") {
    const count = await dumpToolDeclarations();
    process.stdout.write(`
##### 工具声明 ${count} 条 #####
`);
    continue;
  }
  if (name === "absence") {
    const probes = await runGroup(absenceGroup());
    process.stdout.write(`
##### absence: ${probes} 个探针 #####
`);
    continue;
  }
  process.stdout.write(`未知组名：${name}
`);
}
