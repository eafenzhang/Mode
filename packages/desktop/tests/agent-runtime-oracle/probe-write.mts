// M2 Task 3 探针：写工具（Write / Edit）是否注册、接口面、结果形状与**审批落盘**。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-write.mts [surface|call|perm|project]
//
// 纪律（docs/specs/clean-room-policy.md §黑盒对拍）：只**运行**旧实现、观察它对外声明的接口面
// （模型请求里的 `tools[].input_schema`）、脚本化调用后的结果与落盘，不读实现体。
// 表结构同样不读迁移文件（那是实现体）——`local_setting` 的形状来自本探针的观察。
//
// 为什么先探「注册了没有」：Task 2 的实测教训——运行时**对外声明** 21 条工具，但会话面**只注册
// 了 `Read`**（`Glob`/`Grep` 调用得 `tool_not_found`）。所以本探针第一件事就是把写工具也探一遍，
// 声明与注册是两回事。
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { seedWorkspace, startOracleRuntime } from "./driver.mjs";
import { describeOutcome, drain, rawToolOutcome, type MockStep } from "./probe-support.mjs";
import {
  dumpLocalSetting,
  optionResponse,
  printTree,
  runWriteSession,
} from "./probe-write-support.mjs";

/** 声明侧接口面：模型请求里 `tools[]` 的名字与输入 schema，按关注工具过滤。 */
async function surfaceGroup(): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  const wanted = new Set(["Write", "Edit", "ApplyPatch", "MultiEdit", "NotebookEdit", "Bash"]);
  const declared: { name: string; description?: string; input_schema?: unknown }[] = [];
  try {
    const session = await startOracleRuntime({
      dataRoot,
      cwd,
      script: [{ text: "surface probe done" }],
      onMockRequestBody: (_path, body) => {
        const tools = (body as { tools?: unknown[] }).tools;
        if (!Array.isArray(tools) || tools.length === 0) return;
        for (const raw of tools) {
          const tool = raw as { name?: unknown; description?: unknown; input_schema?: unknown };
          const name = String(tool.name ?? "");
          if (!wanted.has(name)) continue;
          declared.push({
            name,
            description: typeof tool.description === "string" ? tool.description : undefined,
            input_schema: tool.input_schema,
          });
        }
      },
    });
    try {
      await session.send("surface probe");
      await drain(session.events);
      process.stdout.write(`\n########## surface：关注工具的声明（cwd=${cwd}）##########\n`);
      const seen = new Set<string>();
      for (const tool of declared) {
        if (seen.has(tool.name)) continue;
        seen.add(tool.name);
        process.stdout.write(`\n== 声明 ${tool.name} ==\n`);
        process.stdout.write(`description: ${JSON.stringify(tool.description)}\n`);
        process.stdout.write(`input_schema: ${JSON.stringify(tool.input_schema, null, 2)}\n`);
      }
      process.stdout.write(`\n（命中的声明工具：${[...seen].join(",") || "无"}）\n`);
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

type CallProbe = { label: string; tool: string; args: Record<string, unknown> };

/**
 * 审批面：脚本化一次 `Write`，把**运行时的反向请求原样录下来**（`interaction/requestPermission`），
 * 并记录 host 侧的应答与数据根里新出现的文件。
 *
 * 观察口是现成的：`onRawFrame` 看入向帧、`onRawWrite` 看 host 发出的那一半——不需要改 driver。
 */
async function permGroup(): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  const inbound: Record<string, unknown>[] = [];
  const outbound: Record<string, unknown>[] = [];
  try {
    const session = await startOracleRuntime({
      dataRoot,
      cwd,
      script: [
        { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/perm.txt", content: "x\n" } }] },
        { text: "perm probe done" },
      ],
      onRawFrame: (frame) => {
        if (typeof frame.method === "string" && frame.id !== undefined) inbound.push(frame);
      },
      onRawWrite: (message) => outbound.push(message),
    });
    try {
      await session.send("perm probe");
      await drain(session.events);
      process.stdout.write(`\n########## perm：反向请求（运行时→host）##########\n`);
      for (const frame of inbound) {
        process.stdout.write(`\nmethod: ${String(frame.method)}\n`);
        process.stdout.write(`params: ${JSON.stringify(frame.params, null, 2)}\n`);
      }
      process.stdout.write(`\n########## perm：host 应答（host→运行时）##########\n`);
      for (const message of outbound) {
        process.stdout.write(`${JSON.stringify(message)}\n`);
      }
      process.stdout.write(`\n########## perm：数据根新增文件 ##########\n`);
      await printTree(dataRoot);
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

/**
 * 「项目内一直允许」的落盘对拍：第一次按 `allow_project` 应答，第二次（同 dataRoot/cwd、新会话）
 * 再做同样的 `Write`，看审批请求是否被已有规则抑制。
 *
 * 两次都先 `Read` 目标文件——否则第二步会先撞「未读就写」的前置条件（实测：`perm.txt` 首轮已建，
 * 次轮直接写拿到的是 `write_file_not_read`，**根本没走到审批**，会得出假的「没有请求」）。
 */
async function projectGroup(): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  try {
    await seedWorkspace(cwd, { "perm.txt": "old\n" });
    await writeAndReport({ dataRoot, cwd, optionId: "allow_project", label: "first" });
    process.stdout.write("\n########## project：第一次（allow_project）后的树 ##########\n");
    await printTree(cwd, "cwd");
    await printTree(dataRoot, "dataRoot");
    await dumpLocalSetting(dataRoot);
    await writeAndReport({ dataRoot, cwd, optionId: null, label: "second" });
    process.stdout.write("\n########## project：第二次（同路径、新会话）后的树 ##########\n");
    await printTree(cwd, "cwd");
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

/**
 * 跑一次「先 Read 再 Write `perm.txt`」，按 `optionId` 回报审批。
 * `optionId === null` 时**不给审批应答**——用它检验「已有的 allow 规则是否抑制了这次请求」：
 * 若仍收到请求，函数会在超时前显式失败（不静默）。
 */
async function writeAndReport(options: {
  dataRoot: string;
  cwd: string;
  optionId: string | null;
  label: string;
}): Promise<void> {
  const requests: Record<string, unknown>[] = [];
  await runWriteSession({
    dataRoot: options.dataRoot,
    cwd: options.cwd,
    relativePath: "perm.txt",
    content: `${options.label}\n`,
    label: options.label,
    onPermissionRequest: (params) => requests.push(params),
    ...(options.optionId === null
      ? {}
      : {
          permissionReply: (params: Record<string, unknown>) =>
            optionResponse(params, options.optionId as string),
        }),
  });
  process.stdout.write(
    `\n== ${options.label}：审批请求 ${requests.length} 次，回报 ${
      options.optionId === null ? "（不应答）" : `optionId=${options.optionId}`
    } ==\n`,
  );
}

/** 调用侧：把每个探针当一个 tool_call 喂进去，打印结果与调用后的工作区文件。 */
async function callGroup(): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  const probes: CallProbe[] = [
    {
      label: "write-new",
      tool: "Write",
      args: { file_path: "${cwd}/made.txt", content: "hello write\nsecond line\n" },
    },
    {
      label: "edit-existing",
      tool: "Edit",
      args: {
        file_path: "${cwd}/seed.txt",
        old_string: "Beta",
        new_string: "BETA",
      },
    },
    {
      label: "apply-patch",
      tool: "ApplyPatch",
      args: { patch: "*** Begin Patch\n*** End Patch\n" },
    },
    { label: "write-positive-control", tool: "Write", args: {} },
  ];
  try {
    await seedWorkspace(cwd, { "seed.txt": "alpha\nBeta\ngamma\n" });
    // `${cwd}` 占位符由 driver 内部用 `substituteCwd` 换（它对 Windows 反斜杠安全），
    // 这里不要自己 replaceAll——上一版就是栽在 JSON 字符串里塞进裸路径。
    const script: MockStep[] = [
      ...probes.map((probe) => ({
        toolCalls: [{ name: probe.tool, input: probe.args }],
      })),
      { text: "call probe done" },
    ];
    const session = await startOracleRuntime({ dataRoot, cwd, script });
    try {
      await session.send("call probe");
      await drain(session.events);
      process.stdout.write(`\n########## call：写工具调用结果（cwd=${cwd}）##########\n`);
      for (const [index, probe] of probes.entries()) {
        const observed = rawToolOutcome(session.frames, index);
        process.stdout.write(`\n== ${index}: ${probe.label}（${probe.tool}）==\n`);
        process.stdout.write(`args: ${JSON.stringify(probe.args)}\n`);
        if (!observed) {
          process.stdout.write("(未观察到结果)\n");
          continue;
        }
        process.stdout.write(`ok: ${observed.ok}\n`);
        process.stdout.write(`${describeOutcome(observed)}\n`);
      }
      process.stdout.write("\n########## call：调用后的工作区 ##########\n");
      for (const name of ["made.txt", "seed.txt"]) {
        const target = join(cwd, name);
        const exists = await stat(target)
          .then(() => true)
          .catch(() => false);
        if (!exists) {
          process.stdout.write(`\n== ${name} == (不存在)\n`);
          continue;
        }
        process.stdout.write(`\n== ${name} ==\n${JSON.stringify(await readFile(target, "utf8"))}\n`);
      }
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

const requested = process.argv.slice(2);
const names = requested.length > 0 ? requested : ["surface", "call", "perm", "project"];
for (const name of names) {
  if (name === "surface") await surfaceGroup();
  else if (name === "call") await callGroup();
  else if (name === "perm") await permGroup();
  else if (name === "project") await projectGroup();
  else process.stdout.write(`未知组名：${name}\n`);
}
