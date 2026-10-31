/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 checkpoint 用例，拆分会降低可核对性。 */
/**
 * 生成 `git-checkpoint-service` 与 TS 真源的对拍数据（真实 git 仓库 + 真实数据目录）。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/git/gitCheckpointService.ts`
 * - `packages/services/src/git/repo/gitCheckpointRepo.ts` / `gitCheckpointStore.ts` / `gitCheckpointHelpers.ts`
 *
 * 归一化：sandbox root → `<root>`；checkpoint id → `<checkpoint-N>`；createdAt → `<ts>`；
 * commit oid → `<hash>`；ref 里的 workspace hash → `<workspaceHash>`。
 *
 * ```bash
 * pnpm git-checkpoint:fixtures
 * cargo test -p git-checkpoint-service
 * ```
 */
import { execFileSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "git-checkpoint-service",
  "fixtures",
  "git-checkpoint-service.json",
);
const SANDBOX = realpathSync.native(
  mkdtempSync(path.join(tmpdir(), "zc-git-checkpoint-fixtures-")),
);

interface FileSpec {
  path: string;
  content: string;
}

type Op =
  | { kind: "write"; path: string; content: string }
  | { kind: "remove"; path: string }
  | { kind: "renameFile"; from: string; to: string }
  | { kind: "read"; path: string }
  | { kind: "create" }
  | { kind: "diff"; from: number; to: number }
  | { kind: "restore"; from: number; to: number; force?: boolean }
  | { kind: "delete"; checkpoint: number }
  | { kind: "refs" };

interface FixtureCase {
  name: string;
  workspace: string;
  setup: {
    files: FileSpec[];
    extraFiles?: FileSpec[];
    skipRepo?: boolean;
  };
  ops: Op[];
}

const GIT_ENV = {
  GIT_AUTHOR_NAME: "Mode Test",
  GIT_AUTHOR_EMAIL: "mode@test.local",
  GIT_COMMITTER_NAME: "Mode Test",
  GIT_COMMITTER_EMAIL: "mode@test.local",
  GIT_AUTHOR_DATE: "2024-01-01T00:00:00+0000",
  GIT_COMMITTER_DATE: "2024-01-01T00:00:00+0000",
  GIT_CONFIG_NOSYSTEM: "1",
};

function applyGitEnv(): void {
  process.env.GIT_CONFIG_GLOBAL = path.join(SANDBOX, "gitconfig-empty");
  process.env.GIT_CONFIG_SYSTEM = path.join(SANDBOX, "gitconfig-empty");
  for (const [key, value] of Object.entries(GIT_ENV)) process.env[key] = value;
}

function gitEnv(): NodeJS.ProcessEnv {
  return {
    ...process.env,
    ...GIT_ENV,
    GIT_CONFIG_GLOBAL: path.join(SANDBOX, "gitconfig-empty"),
    GIT_CONFIG_SYSTEM: path.join(SANDBOX, "gitconfig-empty"),
  };
}

function runGit(cwd: string, args: string[]): string {
  return execFileSync("git", args, {
    cwd,
    env: gitEnv(),
    stdio: ["ignore", "pipe", "pipe"],
  }).toString();
}

function materialize(root: string, files: FileSpec[]): void {
  for (const file of files) {
    const target = path.join(root, file.path);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, file.content);
  }
}

function workspacePath(testCase: FixtureCase): string {
  return path.join(SANDBOX, ...testCase.workspace.split("/"));
}

function repoRootPath(): string {
  return path.join(SANDBOX, "ws");
}

function ensureSandbox(): void {
  rmSync(SANDBOX, { recursive: true, force: true });
  mkdirSync(SANDBOX, { recursive: true });
  writeFileSync(path.join(SANDBOX, "gitconfig-empty"), "");
}

function setupRepo(testCase: FixtureCase): void {
  const repoRoot = repoRootPath();
  mkdirSync(repoRoot, { recursive: true });
  if (testCase.setup.skipRepo) {
    materialize(workspacePath(testCase), testCase.setup.files);
    return;
  }
  runGit(repoRoot, ["init", "-b", "main"]);
  runGit(repoRoot, ["config", "user.name", "Mode Test"]);
  runGit(repoRoot, ["config", "user.email", "mode@test.local"]);
  materialize(repoRoot, testCase.setup.files);
  runGit(repoRoot, ["add", "-A"]);
  runGit(repoRoot, ["commit", "-m", "initial"]);
  if (testCase.setup.extraFiles) {
    materialize(repoRoot, testCase.setup.extraFiles);
  }
}

function listFiles(root: string): Array<{ path: string; content: string }> {
  const entries: Array<{ path: string; content: string }> = [];
  const walk = (dir: string, prefix: string): void => {
    for (const name of readdirSync(dir).sort()) {
      if (name === ".git") continue;
      const absolute = path.join(dir, name);
      const relative = prefix ? `${prefix}/${name}` : name;
      const stats = statSync(absolute);
      if (stats.isDirectory()) {
        walk(absolute, relative);
        continue;
      }
      entries.push({ path: relative, content: readFileSync(absolute, "utf-8") });
    }
  };
  walk(root, "");
  return entries;
}

function normalizeText(root: string, text: string, checkpointIds: Map<string, string>): string {
  const native = root;
  const forward = root.split(path.sep).join("/");
  const escaped = native.split("\\").join("\\\\");
  let replaced = text.replaceAll(escaped, "<root>");
  replaced = replaced.replaceAll(native, "<root>");
  replaced = replaced.replaceAll(forward, "<root>");
  for (const [id, placeholder] of checkpointIds) {
    replaced = replaced.replaceAll(id, placeholder);
  }
  if (path.sep === "\\") {
    replaced = replaced.split("\\\\").join("/").split("\\").join("/");
  }
  replaced = replaced.replace(
    /refs\/mode\/checkpoints\/[0-9a-f]{12}\//gu,
    "refs/mode/checkpoints/<workspaceHash>/",
  );
  return replaced;
}

const HASH = /^[0-9a-f]{40}$/u;

function normalizeValue(root: string, checkpointIds: Map<string, string>, value: unknown): unknown {
  if (typeof value === "string") {
    const replaced = normalizeText(root, value, checkpointIds);
    return HASH.test(replaced) ? "<hash>" : replaced;
  }
  if (Array.isArray(value)) {
    return value.map((item) => normalizeValue(root, checkpointIds, item));
  }
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [childKey, child] of Object.entries(value as Record<string, unknown>)) {
      if (childKey === "createdAt") {
        out[childKey] = "<ts>";
        continue;
      }
      out[childKey] = normalizeValue(root, checkpointIds, child);
    }
    return out;
  }
  return value;
}

const cases: FixtureCase[] = [
  {
    name: "create-diff-restore",
    workspace: "ws",
    setup: {
      files: [
        { path: "a.md", content: "line1\n" },
        { path: "dir/b.md", content: "b1\n" },
      ],
    },
    ops: [
      { kind: "create" },
      { kind: "write", path: "a.md", content: "line1\nline2\n" },
      { kind: "write", path: "new.md", content: "hello\n" },
      { kind: "create" },
      { kind: "diff", from: 0, to: 1 },
      // 把磁盘退回 cp0 基线，再恢复到 cp1。
      { kind: "write", path: "a.md", content: "line1\n" },
      { kind: "remove", path: "new.md" },
      { kind: "restore", from: 0, to: 1 },
    ],
  },
  {
    name: "restore-conflicts",
    workspace: "ws",
    setup: {
      files: [{ path: "a.md", content: "line1\n" }],
    },
    ops: [
      { kind: "create" },
      { kind: "write", path: "a.md", content: "line1\nline2\n" },
      { kind: "write", path: "new.md", content: "hello\n" },
      { kind: "create" },
      // 磁盘仍停留在 cp1：a.md 内容偏离 cp0，new.md 是 cp0 里不存在的新文件。
      { kind: "restore", from: 0, to: 1 },
      { kind: "restore", from: 0, to: 1, force: true },
    ],
  },
  {
    name: "rename-restore",
    workspace: "ws",
    setup: {
      files: [
        { path: "old.md", content: "line1\nline2\nline3\nline4\nline5\n" },
        { path: "keep.md", content: "keep\n" },
      ],
    },
    ops: [
      { kind: "create" },
      { kind: "renameFile", from: "old.md", to: "renamed.md" },
      { kind: "create" },
      { kind: "diff", from: 0, to: 1 },
      { kind: "renameFile", from: "renamed.md", to: "old.md" },
      { kind: "restore", from: 0, to: 1 },
    ],
  },
  {
    name: "delete-checkpoint",
    workspace: "ws",
    setup: {
      files: [{ path: "a.md", content: "line1\n" }],
    },
    ops: [
      { kind: "create" },
      { kind: "create" },
      { kind: "refs" },
      { kind: "delete", checkpoint: 0 },
      { kind: "refs" },
      { kind: "diff", from: 0, to: 1 },
      { kind: "diff", from: 1, to: 1 },
    ],
  },
  {
    name: "nested-workspace-scope",
    workspace: "ws/sub",
    setup: {
      files: [
        { path: "root.md", content: "root1\n" },
        { path: "sub/inner.md", content: "inner1\n" },
      ],
    },
    ops: [
      { kind: "create" },
      { kind: "write", path: "inner.md", content: "inner2\n" },
      { kind: "write", path: "added.md", content: "added\n" },
      { kind: "create" },
      { kind: "diff", from: 0, to: 1 },
      // scope 之外的 root.md 不参与 checkpoint；restore 也不应改它。
      { kind: "write", path: "../root.md", content: "root-dirty\n" },
      { kind: "write", path: "inner.md", content: "inner1\n" },
      { kind: "remove", path: "added.md" },
      { kind: "restore", from: 0, to: 1 },
      { kind: "read", path: "../root.md" },
    ],
  },
  {
    name: "not-a-repository",
    workspace: "ws",
    setup: { files: [{ path: "a.md", content: "line1\n" }], skipRepo: true },
    ops: [{ kind: "create" }],
  },
  {
    name: "missing-checkpoint",
    workspace: "ws",
    setup: { files: [{ path: "a.md", content: "line1\n" }] },
    ops: [
      { kind: "create" },
      { kind: "diff", from: 0, to: 1 },
      { kind: "restore", from: 0, to: 0 },
    ],
  },
];

async function main(): Promise<void> {
  applyGitEnv();
  // `paths.ts` 在模块加载时读取 MODE_DATA_BASE_DIR；必须在动态 import 之前设置。
  process.env.MODE_DATA_BASE_DIR = SANDBOX;

  const { createGitCheckpointService } =
    await import("../packages/services/src/git/gitCheckpointService.js");
  type Meta = Awaited<
    ReturnType<ReturnType<typeof createGitCheckpointService>["createCheckpoint"]>
  >;

  ensureSandbox();
  writeFileSync(path.join(SANDBOX, "gitconfig-empty"), "");

  const results: Array<{
    name: string;
    workspace: string;
    setup: FixtureCase["setup"];
    ops: Op[];
    expected: unknown[];
    tree: Array<{ path: string; content: string }>;
  }> = [];

  for (const testCase of cases) {
    ensureSandbox();
    setupRepo(testCase);
    process.env.MODE_DATA_BASE_DIR = SANDBOX;
    const { createGitCheckpointService: freshFactory } =
      await import("../packages/services/src/git/gitCheckpointService.js");
    const service = freshFactory();
    const ws = workspacePath(testCase);
    const checkpoints: Meta[] = [];
    const checkpointIds = new Map<string, string>();
    const expected: unknown[] = [];

    const normalize = (value: unknown): unknown => normalizeValue(SANDBOX, checkpointIds, value);

    for (const op of testCase.ops) {
      try {
        switch (op.kind) {
          case "write": {
            const target = path.join(ws, op.path);
            mkdirSync(path.dirname(target), { recursive: true });
            writeFileSync(target, op.content);
            expected.push({ ok: true });
            break;
          }
          case "remove": {
            rmSync(path.join(ws, op.path), { force: true, recursive: true });
            expected.push({ ok: true });
            break;
          }
          case "renameFile": {
            renameSync(path.join(ws, op.from), path.join(ws, op.to));
            expected.push({ ok: true });
            break;
          }
          case "read": {
            expected.push(normalize({ content: readFileSync(path.join(ws, op.path), "utf-8") }));
            break;
          }
          case "create": {
            const meta = await service.createCheckpoint({ workspacePath: ws });
            checkpointIds.set(meta.checkpointId, `<checkpoint-${checkpoints.length}>`);
            checkpoints.push(meta);
            expected.push(normalize(meta));
            break;
          }
          case "diff": {
            const from = checkpoints[op.from];
            const to = checkpoints[op.to];
            const result = await service.diffCheckpoints({
              workspacePath: ws,
              fromCheckpointId: from?.checkpointId ?? "missing-checkpoint",
              toCheckpointId: to?.checkpointId ?? "missing-checkpoint",
            });
            expected.push(normalize(result));
            break;
          }
          case "restore": {
            const from = checkpoints[op.from];
            const to = checkpoints[op.to];
            const result = await service.restoreBetweenCheckpoints({
              workspacePath: ws,
              fromCheckpointId: from?.checkpointId ?? "missing-checkpoint",
              toCheckpointId: to?.checkpointId ?? "missing-checkpoint",
              force: op.force,
            });
            expected.push(normalize(result));
            break;
          }
          case "delete": {
            const checkpoint = checkpoints[op.checkpoint];
            await service.deleteCheckpoint({
              workspacePath: ws,
              checkpointId: checkpoint?.checkpointId ?? "missing-checkpoint",
            });
            expected.push({ ok: true });
            break;
          }
          case "refs": {
            const output = execFileSync(
              "git",
              ["for-each-ref", "--format=%(refname)", "refs/mode/checkpoints"],
              { cwd: repoRootPath(), env: gitEnv(), stdio: ["ignore", "pipe", "pipe"] },
            ).toString();
            // ref 名里含随机 UUID，原始排序会随每次生成变化；对归一化后的占位符排序。
            const refs = output
              .split("\n")
              .filter((line) => line.length > 0)
              .map((line) => String(normalize(line)))
              .sort();
            expected.push(refs);
            break;
          }
          default: {
            throw new Error(`unknown op: ${String((op as { kind?: string }).kind)}`);
          }
        }
      } catch (error) {
        expected.push(normalize({ error: error instanceof Error ? error.message : String(error) }));
      }
    }

    results.push({
      name: testCase.name,
      workspace: testCase.workspace,
      setup: testCase.setup,
      ops: testCase.ops,
      expected,
      tree: listFiles(ws),
    });
  }

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases: results }, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${results.length} cases)\n`);
  rmSync(SANDBOX, { recursive: true, force: true });
}

await main();
