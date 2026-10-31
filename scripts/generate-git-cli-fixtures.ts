/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 git-cli 解析用例，拆分会降低可核对性。 */
/**
 * 生成 `git-cli` 与 TS 真源的对拍数据（纯解析器，不调用 git 命令）。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/git/repo/gitCliHelpers.ts`
 * - `packages/services/src/git/config.ts`
 *
 * ```bash
 * pnpm git-cli:fixtures
 * cargo test -p git-cli
 * ```
 */
import path from "node:path";
import { fileURLToPath } from "node:url";
import { mkdirSync, writeFileSync } from "node:fs";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "git-cli", "fixtures", "git-cli.json");

interface Op {
  kind: "status" | "numstat" | "config" | "issues" | "diff" | "path";
  stdout?: string;
  result?: Record<string, unknown>;
  path?: string;
  options?: {
    allowedExitCodes?: number[];
    emptySummary?: string;
    binarySummary?: string;
  };
  fn?: "normalize" | "workspace" | "inScope" | "toWorkspaceRelative";
  args?: string[];
}

interface FixtureCase {
  name: string;
  files: Array<{ path: string; content: string }>;
  ops: Op[];
  expected: unknown[];
  tree: Array<{ path: string; content: string }>;
}

function executionResult(overrides: Record<string, unknown>): Record<string, unknown> {
  return {
    binaryPath: "git",
    cwd: ".",
    args: [],
    stdout: "",
    stderr: "",
    exitCode: 0,
    signal: null,
    durationMs: 1,
    timedOut: false,
    timeoutMs: 15000,
    forceKillAttempted: false,
    orphaned: false,
    outputTruncated: false,
    ...overrides,
  };
}

async function main(): Promise<void> {
  const {
    parseGitBranchMutationIssues,
    parseGitConfigValue,
    parseNumstat,
    parseStatusPorcelain,
    toDiffResult,
  } = await import("../packages/services/src/git/repo/gitCliHelpers.js");
  const {
    isPathInWorkspaceScope,
    normalizeGitPath,
    normalizeWorkspaceInRepoPath,
    toWorkspaceRelativeGitPath,
  } = await import("../packages/services/src/git/config.js");

  const runOp = (op: Op): unknown => {
    try {
      switch (op.kind) {
        case "status":
          return parseStatusPorcelain(op.stdout ?? "");
        case "numstat":
          return Object.fromEntries(parseNumstat(op.stdout ?? ""));
        case "config":
          return parseGitConfigValue(op.result as never);
        case "issues":
          return parseGitBranchMutationIssues(op.result as never);
        case "diff":
          return toDiffResult(op.path ?? "", op.result as never, op.options as never);
        case "path": {
          const args = op.args ?? [];
          switch (op.fn) {
            case "normalize":
              return normalizeGitPath(args[0] ?? "");
            case "workspace":
              return normalizeWorkspaceInRepoPath(args[0] ?? "");
            case "inScope":
              return isPathInWorkspaceScope(args[0] ?? "", args[1] ?? "");
            case "toWorkspaceRelative":
              return toWorkspaceRelativeGitPath(args[0] ?? "", args[1] ?? "");
            default:
              throw new Error(`unknown path fn: ${String(op.fn)}`);
          }
        }
        default:
          throw new Error(`unknown op: ${String((op as { kind?: string }).kind)}`);
      }
    } catch (error) {
      return { error: error instanceof Error ? error.message : String(error) };
    }
  };

  const cases: FixtureCase[] = [
    {
      name: "status",
      files: [],
      ops: [
        {
          kind: "status",
          stdout:
            "# branch.head main\u0000# branch.upstream origin/main\u0000# branch.ab +2 -1\u0000? new file.md\u0000" +
            "1 .M N... 100644 100644 100644 a b dir/mod file.md\u0000" +
            "2 R. N... 100644 100644 100644 a b R100 renamed.md\u0000original.md\u0000" +
            "u UU N... 100644 100644 100644 100644 a b c conflict.md\u0000",
        },
        { kind: "status", stdout: "# branch.head (detached)\u0000? only.md\u0000" },
        { kind: "status", stdout: "# branch.ab +0 -0\u0000x unknown\u00001 .M bad\u0000" },
        { kind: "status", stdout: "" },
      ],
      expected: [],
      tree: [],
    },
    {
      name: "numstat",
      files: [],
      ops: [
        {
          kind: "numstat",
          stdout: "3\t0\tadded.md\u0000-\t-\tbin.dat\u00003\t1\t\u0000old.md\u0000new.md\u0000",
        },
        { kind: "numstat", stdout: "1\t2\tdir/a\tb.md\u0000" },
        { kind: "numstat", stdout: "bad\u0000\u0000" },
      ],
      expected: [],
      tree: [],
    },
    {
      name: "config",
      files: [],
      ops: [
        { kind: "config", result: executionResult({ stdout: "global\tfile:/x\tA\tB\n" }) },
        { kind: "config", result: executionResult({ exitCode: 1, stdout: "" }) },
        { kind: "config", result: executionResult({ stdout: "only\ttwo\n" }) },
        { kind: "config", result: executionResult({ stdout: "" }) },
        {
          kind: "config",
          result: executionResult({
            exitCode: null,
            stdout: "",
            stderr: "",
            timedOut: true,
            durationMs: 16000,
            timeoutElapsedMs: 15010,
            timeoutCloseDelayMs: 990,
            forceKillAttempted: true,
            orphaned: true,
          }),
        },
        {
          kind: "config",
          result: executionResult({ outputTruncated: true, stdout: "" }),
        },
      ],
      expected: [],
      tree: [],
    },
    {
      name: "issues",
      files: [],
      ops: [
        {
          kind: "issues",
          result: executionResult({
            exitCode: 1,
            stderr:
              "error: Your local changes to the following files would be overwritten by checkout:\n\tpkg/a.md\nPlease commit your changes.",
          }),
        },
        {
          kind: "issues",
          result: executionResult({
            exitCode: 1,
            stderr:
              "error: The following untracked working tree files would be overwritten by switch:\n\tnew.md\n",
          }),
        },
        {
          kind: "issues",
          result: executionResult({
            exitCode: 128,
            stderr: "fatal: a branch named 'x' already exists",
          }),
        },
        {
          kind: "issues",
          result: executionResult({ exitCode: 128, stderr: "fatal: invalid reference: nope" }),
        },
        {
          kind: "issues",
          result: executionResult({
            exitCode: 128,
            stderr: "fatal: 'x' is already used by worktree at '/tmp/x'",
          }),
        },
        {
          kind: "issues",
          result: executionResult({
            exitCode: 128,
            stderr: "error: you need to resolve your current index first",
          }),
        },
        {
          kind: "issues",
          result: executionResult({
            exitCode: 128,
            stderr: "fatal: cannot switch branch while rebasing",
          }),
        },
        {
          kind: "issues",
          result: executionResult({ exitCode: 128, stderr: "fatal: something else" }),
        },
        { kind: "issues", result: executionResult({ exitCode: 128 }) },
      ],
      expected: [],
      tree: [],
    },
    {
      name: "diff",
      files: [],
      ops: [
        { kind: "diff", path: "a.md", result: executionResult({ timedOut: true }) },
        { kind: "diff", path: "a.md", result: executionResult({ outputTruncated: true }) },
        { kind: "diff", path: "a.md", result: executionResult({ exitCode: 1, stderr: "boom" }) },
        { kind: "diff", path: "a.md", result: executionResult({ stdout: "   " }) },
        {
          kind: "diff",
          path: "a.bin",
          result: executionResult({ stdout: "Binary files a and b differ\n" }),
        },
        { kind: "diff", path: "a.bin", result: executionResult({ stdout: "GIT binary patch\n" }) },
        {
          kind: "diff",
          path: "a.md",
          result: executionResult({ stdout: "diff --git a/a.md b/a.md\n" }),
        },
        {
          kind: "diff",
          path: "a.md",
          result: executionResult({ stdout: "" }),
          options: { emptySummary: "EMPTY", binarySummary: "BIN" },
        },
      ],
      expected: [],
      tree: [],
    },
    {
      name: "paths",
      files: [],
      ops: [
        { kind: "path", fn: "normalize", args: ["a\\b\\c"] },
        { kind: "path", fn: "workspace", args: ["./pkg///"] },
        { kind: "path", fn: "workspace", args: [""] },
        { kind: "path", fn: "inScope", args: ["pkg/a.md", "pkg"] },
        { kind: "path", fn: "inScope", args: ["pkgx/a.md", "pkg"] },
        { kind: "path", fn: "inScope", args: ["anything", "."] },
        { kind: "path", fn: "toWorkspaceRelative", args: ["pkg/a.md", "pkg"] },
        { kind: "path", fn: "toWorkspaceRelative", args: ["pkg", "pkg"] },
        { kind: "path", fn: "toWorkspaceRelative", args: ["pkgx/a.md", "pkg"] },
      ],
      expected: [],
      tree: [],
    },
  ];

  for (const testCase of cases) {
    testCase.expected = testCase.ops.map((op) => runOp(op));
  }

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases }, null, 2)}\n`);
  process.stdout.write(
    `wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases, ${cases.reduce((sum, c) => sum + c.ops.length, 0)} ops)\n`,
  );
}

await main();
