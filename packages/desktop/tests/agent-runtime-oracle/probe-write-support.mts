// M2 Task 3：写工具探针的**共用支撑**（诊断用，不进对拍套件）。
//
// 与 `probe-support.mts` 的分工：那一份服务 `Read` 的入参探针，这一份服务写工具与**审批面**。
//
// 纪律（docs/specs/clean-room-policy.md §黑盒对拍）：只运行旧实现、观察它的输入输出、事件序列与
// **落盘内容**；不读实现体，也不把观察手写进夹具（夹具由 `record.mts` 录）。
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { seedWorkspace, startOracleRuntime } from "./driver.mjs";
import { describeOutcome, drain, rawToolOutcome, type MockStep } from "./probe-support.mjs";

/** 从审批请求里取出某个 optionId 的 `response`（host 侧照抄即得该选项的语义）。 */
export function optionResponse(params: Record<string, unknown>, optionId: string): unknown {
  const options = Array.isArray(params.options) ? params.options : [];
  for (const raw of options) {
    const option = raw as { optionId?: unknown; response?: unknown };
    if (option.optionId === optionId) return option.response;
  }
  throw new Error(`审批请求里没有 optionId=${optionId}；实收：${JSON.stringify(options)}`);
}

/** 递归列出目录树；只打印路径，`settings`/`permission` 相关文件额外打印内容。 */
export async function printTree(root: string, prefix = ""): Promise<void> {
  const { readdir } = await import("node:fs/promises");
  const entries = await readdir(root, { withFileTypes: true }).catch(() => []);
  for (const entry of entries) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) {
      process.stdout.write(`[dir]  ${relative}\n`);
      await printTree(join(root, entry.name), relative);
      continue;
    }
    process.stdout.write(`[file] ${relative}\n`);
    if (/settings|permission|approv|allow/i.test(relative)) {
      const text = await readFile(join(root, entry.name), "utf8").catch(() => "");
      if (text.length <= 4000) process.stdout.write(`--- ${relative} ---\n${text}\n`);
      else process.stdout.write(`--- ${relative} --- (${text.length} 字符，略)\n`);
    }
  }
}

/**
 * 把运行时 DB 的 `local_setting` 表原样打出来——**这就是审批三态的落盘处**（实测）。
 *
 * 实测（2026-10-17，`probe-write.mts project`）：`allow_project` 不写任何文件，
 * 而是写 `.mode/cli/db/db.sqlite` 的 `local_setting` 一行：
 * `scope=project`、`scope_id=proj_<slug>`、`namespace=permission`、`key=ruleset`、
 * `value={"version":1,"allow":[{"toolName","ruleContent"}]}`、`schema_version=1`。
 * 只读打开，不做任何写操作。
 */
export async function dumpLocalSetting(dataRoot: string): Promise<void> {
  const { DatabaseSync } = await import("node:sqlite");
  const dbPath = join(dataRoot, ".mode", "cli", "db", "db.sqlite");
  let db: InstanceType<typeof DatabaseSync>;
  try {
    db = new DatabaseSync(dbPath, { readOnly: true });
  } catch (error) {
    process.stdout.write(`\n== db 打不开：${String(error)} ==\n`);
    return;
  }
  const tables = db
    .prepare("select name from sqlite_master where type='table'")
    .all() as { name: string }[];
  process.stdout.write(`\n== db 表：${tables.map((t) => t.name).join(",")} ==\n`);
  for (const name of ["local_setting", "permission"]) {
    if (!tables.some((t) => t.name === name)) continue;
    const rows = db.prepare(`select * from "${name}"`).all();
    process.stdout.write(`\n== 表 ${name} ==\n${JSON.stringify(rows, null, 2)}\n`);
  }
  db.close();
}

/** 一次「先 Read 再 Write 同一路径」的会话；`permissionReply` 决定审批怎么答。 */
export async function runWriteSession(options: {
  dataRoot: string;
  cwd: string;
  relativePath: string;
  content: string;
  label: string;
  permissionReply?: (params: Record<string, unknown>) => unknown;
  /** 观测钩子：每收到一次 `interaction/requestPermission` 回调一次。 */
  onPermissionRequest?: (params: Record<string, unknown>) => void;
}): Promise<void> {
  const session = await startOracleRuntime({
    dataRoot: options.dataRoot,
    cwd: options.cwd,
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: `\${cwd}/${options.relativePath}` } }] },
      {
        toolCalls: [
          {
            name: "Write",
            input: { file_path: `\${cwd}/${options.relativePath}`, content: options.content },
          },
        ],
      },
      { text: `${options.label} done` },
    ],
    onRawFrame: (frame) => {
      if (frame.method === "interaction/requestPermission") {
        options.onPermissionRequest?.(frame.params as Record<string, unknown>);
      }
    },
    ...(options.permissionReply ? { permissionReply: options.permissionReply } : {}),
  });
  try {
    await session.send(`${options.label} write`);
    await drain(session.events);
    const outcome = rawToolOutcome(session.frames, 1);
    if (outcome) process.stdout.write(`  ${describeOutcome(outcome)}\n`);
    const target = join(options.cwd, options.relativePath);
    const exists = await stat(target)
      .then(() => true)
      .catch(() => false);
    process.stdout.write(
      `  ${options.relativePath} ${exists ? `= ${JSON.stringify(await readFile(target, "utf8"))}` : "（不存在）"}\n`,
    );
  } finally {
    await session.stop();
  }
}

/** 起一对隔离目录并跑一段回调（探针共用）；结束即清理。 */
export async function withIsolatedRoots(
  body: (roots: { dataRoot: string; cwd: string }) => Promise<void>,
  seed: Record<string, string> = {},
): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  try {
    await seedWorkspace(cwd, seed);
    await body({ dataRoot, cwd });
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

export type { MockStep };
