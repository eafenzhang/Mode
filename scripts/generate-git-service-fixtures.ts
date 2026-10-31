/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 git 用例，拆分会降低可核对性。 */
/**
 * 生成 `git-service`（repo 层）与 TS 真源的对拍数据（真实 git 仓库）。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/git/repo/gitCliRepo.ts`
 *
 * 固定作者与提交时间、隔离全局配置；commit hash 在归一化阶段替换为 `<hash>`。
 *
 * ```bash
 * pnpm git-service:fixtures
 * cargo test -p git-service
 * ```
 */
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "git-service", "fixtures", "git-service.json");
const SANDBOX = realpathSync.native(mkdtempSync(path.join(tmpdir(), "zc-git-service-fixtures-")));

interface FileSpec {
  path: string;
  content: string;
}

interface Op {
  kind:
    | "status"
    | "ignored"
    | "branches"
    | "graph"
    | "identity"
    | "diff"
    | "switch"
    | "createBranch"
    | "stage"
    | "unstage"
    | "discard"
    | "commit"
    | "push"
    | "repoInfo"
    | "branchComparison"
    | "changes"
    | "summary"
    | "refresh"
    | "commitService"
    | "generate";
  paths?: string[];
  path?: string;
  sourceId?: "unstaged" | "staged" | "branch";
  staged?: boolean;
  branch?: string;
  startPoint?: string;
  message?: string;
  maxCount?: number;
  skip?: number;
  stagedOnly?: boolean;
  includeIdentity?: boolean;
  includeBranchComparison?: boolean;
}

interface FixtureCase {
  name: string;
  setup: {
    files: FileSpec[];
    remote?: boolean;
    branch?: string;
    detach?: boolean;
    extraFiles?: FileSpec[];
    addPaths?: string[];
    skipRepo?: boolean;
  };
  ops: Op[];
  expected: unknown[];
  tree: Array<{ path: string; content: string }>;
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

function workspacePath(): string {
  return path.join(SANDBOX, "ws");
}

function ensureSandbox(): void {
  rmSync(SANDBOX, { recursive: true, force: true });
  mkdirSync(SANDBOX, { recursive: true });
  writeFileSync(path.join(SANDBOX, "gitconfig-empty"), "");
}

function setupRepo(testCase: FixtureCase): void {
  const ws = workspacePath();
  mkdirSync(ws, { recursive: true });
  if (testCase.setup.skipRepo) {
    materialize(ws, testCase.setup.files);
    return;
  }
  runGit(ws, ["init", "-b", "main"]);
  runGit(ws, ["config", "user.name", "Mode Test"]);
  runGit(ws, ["config", "user.email", "mode@test.local"]);
  materialize(ws, testCase.setup.files);
  runGit(ws, ["add", "-A"]);
  runGit(ws, ["commit", "-m", "initial"]);
  if (testCase.setup.remote) {
    const remote = path.join(SANDBOX, "remote.git");
    runGit(SANDBOX, ["init", "--bare", remote]);
    runGit(ws, ["remote", "add", "origin", remote]);
    runGit(ws, ["push", "-u", "origin", "main"]);
  }
  if (testCase.setup.branch) {
    runGit(ws, ["branch", testCase.setup.branch]);
  }
  if (testCase.setup.extraFiles) {
    materialize(ws, testCase.setup.extraFiles);
  }
  if (testCase.setup.addPaths && testCase.setup.addPaths.length > 0) {
    runGit(ws, ["add", "--", ...testCase.setup.addPaths]);
  }
  if (testCase.setup.detach) {
    runGit(ws, ["checkout", "--detach"]);
  }
}

function snapshotJson(snapshot: {
  resolution: unknown;
  summary: unknown;
  entries: unknown;
  stagedStats: Map<string, unknown>;
  unstagedStats: Map<string, unknown>;
  untrackedStats: Map<string, unknown>;
}): unknown {
  return {
    resolution: snapshot.resolution,
    summary: snapshot.summary,
    entries: snapshot.entries,
    stagedStats: Object.fromEntries(snapshot.stagedStats),
    unstagedStats: Object.fromEntries(snapshot.unstagedStats),
    untrackedStats: Object.fromEntries(snapshot.untrackedStats),
  };
}

function normalizeText(root: string, text: string): string {
  const native = root;
  const forward = root.split(path.sep).join("/");
  const escaped = native.split("\\").join("\\\\");
  let replaced = text.replaceAll(escaped, "<root>");
  replaced = replaced.replaceAll(native, "<root>");
  replaced = replaced.replaceAll(forward, "<root>");
  if (path.sep === "\\") {
    replaced = replaced.split("\\\\").join("/").split("\\").join("/");
  }
  return replaced;
}

const HASH = /^[0-9a-f]{40}$/u;

function normalizeValue(root: string, value: unknown, key?: string): unknown {
  if (typeof value === "string") {
    if (key === "patch") return value;
    const replaced = normalizeText(root, value);
    return HASH.test(replaced) ? "<hash>" : replaced;
  }
  if (Array.isArray(value)) return value.map((item) => normalizeValue(root, item));
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [childKey, child] of Object.entries(value as Record<string, unknown>)) {
      out[childKey] = normalizeValue(root, child, childKey);
    }
    return out;
  }
  return value;
}

async function main(): Promise<void> {
  process.env.GIT_CONFIG_GLOBAL = path.join(SANDBOX, "gitconfig-empty");
  process.env.GIT_CONFIG_SYSTEM = path.join(SANDBOX, "gitconfig-empty");
  process.env.GIT_CONFIG_NOSYSTEM = "1";
  for (const [key, value] of Object.entries(GIT_ENV)) process.env[key] = value;

  const { createGitCliRepo } = await import("../packages/services/src/git/repo/gitCliRepo.js");
  const { createGitService } = await import("../packages/services/src/git/gitService.js");

  const BASE_FILES: FileSpec[] = [
    { path: "a.md", content: "line1\n" },
    { path: "dir/b.md", content: "b1\n" },
    { path: ".gitignore", content: "ignored.txt\n" },
  ];

  const runOp = async (
    repo: ReturnType<typeof createGitCliRepo>,
    service: ReturnType<typeof createGitService>,
    op: Op,
  ): Promise<unknown> => {
    try {
      const ws = workspacePath();
      switch (op.kind) {
        case "status":
          return snapshotJson(await repo.getStatus(ws));
        case "ignored":
          return await repo.getIgnoredPaths(ws, op.paths ?? []);
        case "branches":
          return await repo.listLocalBranches(ws);
        case "graph":
          return await repo.getCommitGraph(ws, op.maxCount, op.skip);
        case "identity":
          return await repo.getIdentity(ws);
        case "diff":
          return await repo.getDiff({
            workspacePath: ws,
            path: op.path ?? "",
            ...(op.sourceId ? { sourceId: op.sourceId } : {}),
            ...(op.staged !== undefined ? { staged: op.staged } : {}),
          });
        case "switch":
          return await repo.switchBranch(ws, op.branch ?? "");
        case "createBranch":
          return await repo.createBranchAndSwitch(ws, op.branch ?? "", op.startPoint);
        case "stage":
          await repo.stage(ws, op.paths ?? []);
          return { ok: true };
        case "unstage":
          await repo.unstage(ws, op.paths ?? []);
          return { ok: true };
        case "discard":
          await repo.discard(ws, op.paths ?? [], op.staged ?? false);
          return { ok: true };
        case "commit":
          return await repo.commit(ws, op.message ?? "", op.paths, {
            stagedOnly: op.stagedOnly,
          });
        case "push":
          return await repo.push(ws);
        case "repoInfo":
          return await repo.getWorkspaceRepositoryInfo(ws);
        case "branchComparison":
          return await repo.getBranchComparison(ws);
        case "changes":
          return await service.getChanges({
            workspacePath: ws,
            sourceId: op.sourceId ?? "unstaged",
          });
        case "summary":
          return await service.getRepositorySummary({ workspacePath: ws });
        case "refresh":
          return await service.refresh({
            workspacePath: ws,
            includeIdentity: op.includeIdentity,
            includeBranchComparison: op.includeBranchComparison,
          });
        case "commitService":
          return await service.commit({
            workspacePath: ws,
            message: op.message ?? "",
            paths: op.paths,
            stagedOnly: op.stagedOnly,
          });
        case "generate":
          return await service.generateCommitMessage({ workspacePath: ws });
        default:
          throw new Error(`unknown op: ${String((op as { kind?: string }).kind)}`);
      }
    } catch (error) {
      return { error: error instanceof Error ? error.message : String(error) };
    }
  };

  const cases: FixtureCase[] = [
    {
      name: "status-diff-branches",
      setup: {
        files: BASE_FILES,
        branch: "feature",
        extraFiles: [
          { path: "a.md", content: "line1\nline2\n" },
          { path: "dir/b.md", content: "b1\nb2\n" },
          { path: "new.md", content: "hello\n" },
          { path: "ignored.txt", content: "ignored\n" },
        ],
        addPaths: ["dir/b.md"],
      },
      ops: [
        { kind: "status" },
        { kind: "ignored", paths: ["a.md", "ignored.txt", "new.md", "../escape"] },
        { kind: "branches" },
        { kind: "graph", maxCount: 10 },
        { kind: "identity" },
        { kind: "repoInfo" },
        { kind: "diff", path: "a.md" },
        { kind: "diff", path: "dir/b.md", staged: true },
        { kind: "diff", path: "new.md" },
        { kind: "diff", path: "a.md", sourceId: "branch" },
      ],
      expected: [],
      tree: [],
    },
    {
      name: "branch-mutations",
      setup: { files: BASE_FILES, branch: "feature" },
      ops: [
        { kind: "branches" },
        { kind: "switch", branch: "feature" },
        { kind: "switch", branch: "feature" },
        { kind: "switch", branch: "" },
        { kind: "switch", branch: "missing" },
        { kind: "createBranch", branch: "topic" },
        { kind: "createBranch", branch: "" },
        { kind: "createBranch", branch: "bad name" },
        { kind: "branches" },
      ],
      expected: [],
      tree: [],
    },
    {
      name: "commit-push",
      setup: {
        files: BASE_FILES,
        remote: true,
        extraFiles: [
          { path: "a.md", content: "line1\nline2\n" },
          { path: "new.md", content: "hello\n" },
        ],
        addPaths: ["a.md", "new.md"],
      },
      ops: [
        { kind: "status" },
        { kind: "branchComparison" },
        { kind: "commit", message: "feat: change" },
        { kind: "status" },
        { kind: "branchComparison" },
        { kind: "push" },
        { kind: "status" },
        { kind: "branchComparison" },
      ],
      expected: [],
      tree: [],
    },
    {
      name: "commit-staged-only",
      setup: {
        files: BASE_FILES,
        remote: true,
        extraFiles: [
          { path: "a.md", content: "line1\nline2\n" },
          { path: "dir/b.md", content: "b1\nb2\n" },
        ],
        addPaths: ["a.md", "dir/b.md"],
      },
      ops: [
        { kind: "status" },
        { kind: "commit", message: "feat: selected", paths: ["a.md"], stagedOnly: true },
        { kind: "status" },
        { kind: "discard", paths: ["dir/b.md"], staged: true },
        { kind: "status" },
      ],
      expected: [],
      tree: [],
    },
    {
      name: "facade",
      setup: {
        files: BASE_FILES,
        remote: true,
        extraFiles: [
          { path: "a.md", content: "line1\nline2\n" },
          { path: "dir/b.md", content: "b1\nb2\n" },
          { path: "new.md", content: "hello\n" },
        ],
        addPaths: ["dir/b.md"],
      },
      ops: [
        { kind: "changes", sourceId: "unstaged" },
        { kind: "changes", sourceId: "staged" },
        { kind: "summary" },
        { kind: "refresh", includeIdentity: true, includeBranchComparison: true },
        { kind: "commitService", message: "feat: facade", paths: ["a.md", "new.md"] },
        { kind: "changes", sourceId: "unstaged" },
        { kind: "changes", sourceId: "staged" },
        { kind: "generate" },
      ],
      expected: [],
      tree: [],
    },
    {
      name: "not-a-repository",
      setup: { files: [], skipRepo: true },
      ops: [
        { kind: "status" },
        { kind: "repoInfo" },
        { kind: "branches" },
        { kind: "graph" },
        { kind: "identity" },
        { kind: "ignored", paths: ["a.md"] },
      ],
      expected: [],
      tree: [],
    },
    {
      name: "detached-head",
      setup: { files: BASE_FILES, detach: true },
      ops: [
        { kind: "status" },
        { kind: "branches" },
        { kind: "push" },
        { kind: "switch", branch: "main" },
      ],
      expected: [],
      tree: [],
    },
  ];

  for (const testCase of cases) {
    ensureSandbox();
    setupRepo(testCase);
    const repo = createGitCliRepo();
    const service = createGitService({ repo });
    const expected: unknown[] = [];
    for (const op of testCase.ops) {
      expected.push(normalizeValue(SANDBOX, await runOp(repo, service, op)));
    }
    testCase.expected = expected;
    testCase.tree = [];
  }

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases }, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases)\n`);
  rmSync(SANDBOX, { recursive: true, force: true });
}

await main();
