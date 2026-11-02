/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明工具解析矩阵，拆分会降低可核对性。 */
/**
 * 生成 `runtime-tools` 的 Rust 对拍数据（Windows / x64）。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/runtime-tools/runtimeToolResolver.ts`
 *
 * 每条用例独立沙箱目录；path 里的 `<root>` 在两侧都替换成沙箱根。夹具只覆盖
 * win32-x64（TS 的 `process.platform/arch` 不可注入），POSIX 分支由 Rust 原生
 * `std::path` 保证，不在本夹具口径内。
 *
 * ```bash
 * pnpm runtime-tools:fixtures
 * cargo test -p runtime-tools
 * ```
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildShellBootstrapPath,
  captureLoginShellEnvSnapshot,
} from "../packages/services/src/runtime-tools/runtimeLoginShellEnvCapture.js";
import {
  findModeAgentRuntimeBinary,
  findModeAgentRuntimeNodeBundle,
} from "../packages/services/src/runtime-tools/providerRuntimeResolver.js";
import { resolveClientConfigPlatform } from "../packages/services/src/runtime-tools/clientPlatform.js";
import { buildRuntimeToolEnvPatch } from "../packages/services/src/runtime-tools/runtimeToolResolver.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "runtime-tools", "fixtures", "runtime-tools.json");

interface FixtureCase {
  name: string;
  cwd: string;
  toolIds: string[];
  env: Record<string, string>;
  setup: string[];
}

const CASES: FixtureCase[] = [
  {
    name: "runtime-root",
    cwd: "ws",
    toolIds: ["ripgrep"],
    env: { MODE_SERVER_RUNTIME_ROOT: "<root>/runtime" },
    setup: ["runtime/tools/ripgrep/rg.exe"],
  },
  {
    name: "cwd-bundled-tools",
    cwd: "ws",
    toolIds: ["bfs"],
    env: {},
    setup: ["ws/bundled-tools/win32-x64/bfs/bfs.exe"],
  },
  {
    name: "packages-desktop-bundled-tools",
    cwd: "ws",
    toolIds: ["ugrep"],
    env: {},
    setup: ["ws/packages/desktop/bundled-tools/win32-x64/ugrep/ugrep.exe"],
  },
  {
    name: "sibling-desktop-bundled-tools",
    cwd: "ws",
    toolIds: ["ripgrep"],
    env: {},
    setup: ["desktop/bundled-tools/win32-x64/ripgrep/rg.exe"],
  },
  {
    name: "binary-env-override",
    cwd: "ws",
    toolIds: ["bfs"],
    env: { MODE_BFS_BINARY: " <root>/ws/custom/bfs-bin.exe " },
    setup: ["ws/custom/bfs-bin.exe"],
  },
  {
    name: "invalid-env-falls-back",
    cwd: "ws",
    toolIds: ["bfs"],
    env: { MODE_BFS_BINARY: "<root>/missing/bfs.exe" },
    setup: ["ws/bundled-tools/win32-x64/bfs/bfs.exe"],
  },
  {
    name: "path-fallback-default-ext",
    cwd: "ws",
    toolIds: ["bfs"],
    env: { PATH: "<root>/ws/bin" },
    setup: ["ws/bin/bfs.EXE"],
  },
  {
    name: "path-fallback-pathext",
    cwd: "ws",
    toolIds: ["bfs"],
    env: { PATH: "<root>/ws/bin", PATHEXT: ".CMD" },
    setup: ["ws/bin/bfs.CMD"],
  },
  {
    name: "path-dedupe",
    cwd: "ws",
    toolIds: ["bfs"],
    env: { PATH: "<root>/ws/bin;<root>/ws/bundled-tools/win32-x64/bfs" },
    setup: ["ws/bundled-tools/win32-x64/bfs/bfs.exe"],
  },
  {
    name: "multi-tools",
    cwd: "ws",
    toolIds: ["bfs", "ripgrep", "ugrep"],
    env: { MODE_SERVER_RUNTIME_ROOT: "<root>/runtime" },
    setup: [
      "runtime/tools/bfs/bfs.exe",
      "runtime/tools/ripgrep/rg.exe",
      "runtime/tools/ugrep/ugrep.exe",
    ],
  },
  {
    name: "none-found",
    cwd: "ws",
    toolIds: ["bfs"],
    env: { PATH: "<root>/empty" },
    setup: [],
  },
];

function materialize(root: string, setup: string[]): void {
  for (const relative of setup) {
    const target = path.join(root, relative);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, "");
  }
}

function withRoot<T>(value: T, root: string): T {
  if (typeof value === "string") return value.replaceAll("<root>", root) as T;
  if (Array.isArray(value)) return value.map((item) => withRoot(item, root)) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>).map(([key, child]) => [
        key,
        withRoot(child, root),
      ]),
    ) as T;
  }
  return value;
}

function normalize<T>(value: T, root: string): T {
  const escaped = root.replaceAll("\\", "\\\\");
  return JSON.parse(JSON.stringify(value).replaceAll(escaped, "<root>")) as T;
}

const LOGIN_PREFIX = "__MODE_LOGIN_ENV_START__";
const LOGIN_SUFFIX = "__MODE_LOGIN_ENV_END__";

function snapshot(entries: Array<[string, string]>, extra = ""): string {
  return `noise\0${LOGIN_PREFIX}\0${entries
    .map(([key, value]) => `${key}=${value}`)
    .join("\0")}\0${LOGIN_SUFFIX}\0${extra}`;
}

interface LoginCase {
  name: string;
  baseEnv: Record<string, string>;
  shellPath?: string | null;
  shellFile?: string;
  timeoutMs?: number;
  output: string;
}

const LOGIN_CASES: LoginCase[] = [
  {
    name: "explicit-zsh",
    baseEnv: { PATH: "/custom/bin", TERM: "xterm", FOO: "a=b" },
    shellPath: "/bin/zsh",
    output: snapshot([
      ["PATH", "/home/user/.local/bin:/usr/bin"],
      ["TERM", "xterm-256color"],
      ["EMPTY", ""],
      ["WITH_EQUALS", "a=b=c"],
    ]),
  },
  {
    name: "explicit-sh-invalid-keys",
    baseEnv: {},
    shellPath: "/bin/sh",
    output: snapshot([
      ["1BAD", "x"],
      [" BAD", "x"],
      ["=v", "x"],
      ["OK_KEY", "1"],
    ]),
  },
  {
    name: "resolved-zsh",
    baseEnv: { SHELL: "<root>/shell/zsh" },
    shellFile: "shell/zsh",
    output: snapshot([["FROM_SHELL", "1"]]),
  },
  {
    name: "resolved-custom-shell",
    baseEnv: { SHELL: "<root>/shell/myshell" },
    shellFile: "shell/myshell",
    output: snapshot([["FROM_SHELL", "2"]]),
  },
  {
    name: "explicit-null",
    baseEnv: {},
    shellPath: null,
    output: "",
  },
  {
    name: "no-markers",
    baseEnv: {},
    shellPath: "/bin/sh",
    output: "plain output without markers",
  },
  {
    name: "last-marker-pair",
    baseEnv: {},
    shellPath: "/bin/bash",
    output:
      snapshot([["OLD", "1"]]) + snapshot([["NEW", "2"], ["SECOND", "yes"]]),
  },
  {
    name: "custom-timeout",
    baseEnv: { PATH: "/bin" },
    shellPath: "/bin/zsh",
    timeoutMs: 1234,
    output: snapshot([]),
  },
];

async function runLoginCase(testCase: LoginCase, root: string): Promise<unknown> {
  let recordedArgs: string[] | null = null;
  let recordedOptions: Record<string, unknown> | null = null;
  const env = withRoot(testCase.baseEnv, root);
  const result = await captureLoginShellEnvSnapshot({
    baseEnv: env as never,
    platform: "linux",
    ...(Object.prototype.hasOwnProperty.call(testCase, "shellPath")
      ? { shellPath: testCase.shellPath as never }
      : {}),
    ...(testCase.timeoutMs === undefined ? {} : { timeoutMs: testCase.timeoutMs }),
    executeShell: async (_shellPath, shellArgs, options) => {
      recordedArgs = shellArgs;
      recordedOptions = {
        timeout: options.timeout,
        maxBuffer: options.maxBuffer,
        env: options.env,
      };
      return testCase.output;
    },
  });
  return {
    resolvedShell: recordedArgs === null ? null : null,
    args: recordedArgs,
    options:
      recordedOptions === null
        ? null
        : {
            ...recordedOptions,
            env: normalize(recordedOptions.env as never, root),
          },
    captured: result === null ? null : normalize(result as never, root),
  };
}

interface AgentRuntimeCase {
  name: string;
  cwd: string;
  env: Record<string, string>;
  setup: string[];
}

const AGENT_RUNTIME_CASES: AgentRuntimeCase[] = [
  {
    name: "binary-env-override",
    cwd: "ws",
    env: { GLM_BINARY_PATH: "<root>/bin/mode-agent.exe" },
    setup: ["bin/mode-agent.exe"],
  },
  {
    name: "cwd-bundled-agents",
    cwd: "ws",
    env: {},
    setup: ["ws/bundled-agents/win32-x64/glm/mode-agent.exe"],
  },
  {
    name: "packages-desktop-bundled-agents",
    cwd: "ws",
    env: {},
    setup: ["ws/packages/desktop/bundled-agents/win32-x64/glm/mode-agent.exe"],
  },
  {
    name: "sibling-desktop-bundled-agents",
    cwd: "ws",
    env: {},
    setup: ["desktop/bundled-agents/win32-x64/glm/mode-agent.exe"],
  },
  {
    name: "legacy-cwd-bundled-resources",
    cwd: "ws",
    env: {},
    setup: ["ws/bundled-resources/glm/mode-agent.exe"],
  },
  {
    name: "node-bundle-legacy-cwd",
    cwd: "ws",
    env: {},
    setup: ["ws/bundled-resources/glm/mode.cjs"],
  },
  {
    name: "node-bundle-bundled-agents",
    cwd: "ws",
    env: {},
    setup: ["ws/bundled-agents/win32-x64/glm/mode.cjs"],
  },
  {
    name: "empty-env-falls-back",
    cwd: "ws",
    env: { GLM_BINARY_PATH: "" },
    setup: ["ws/bundled-resources/glm/mode-agent.exe"],
  },
  {
    name: "both-present",
    cwd: "ws",
    env: {},
    setup: [
      "ws/bundled-agents/win32-x64/glm/mode-agent.exe",
      "ws/bundled-agents/win32-x64/glm/mode.cjs",
    ],
  },
  {
    name: "not-found",
    cwd: "ws",
    env: {},
    setup: [],
  },
];

function runAgentRuntimeCase(testCase: AgentRuntimeCase, root: string): unknown {
  const env = withRoot(testCase.env, root);
  const previous = process.env.GLM_BINARY_PATH;
  if (Object.prototype.hasOwnProperty.call(env, "GLM_BINARY_PATH")) {
    process.env.GLM_BINARY_PATH = env.GLM_BINARY_PATH;
  } else {
    delete process.env.GLM_BINARY_PATH;
  }
  try {
    return {
      binary: normalize(findModeAgentRuntimeBinary(), root),
      nodeBundle: normalize(findModeAgentRuntimeNodeBundle(), root),
    };
  } finally {
    if (previous === undefined) delete process.env.GLM_BINARY_PATH;
    else process.env.GLM_BINARY_PATH = previous;
  }
}

interface CaptureCase {
  name: string;
  baseEnv: Record<string, string>;
  platform: string;
  shellPath?: string | null;
  timeoutMs: number;
  behavior: "output" | "throw" | "delay";
  output: string;
  delayMs?: number;
}

const CAPTURE_CASES: CaptureCase[] = [
  {
    name: "success",
    baseEnv: {},
    platform: "linux",
    shellPath: "/bin/sh",
    timeoutMs: 4000,
    behavior: "output",
    output: snapshot([["CAPTURED", "1"]]),
  },
  {
    name: "win32-short-circuit",
    baseEnv: {},
    platform: "win32",
    shellPath: "/bin/sh",
    timeoutMs: 4000,
    behavior: "output",
    output: snapshot([["SHOULD_NOT_RUN", "1"]]),
  },
  {
    name: "vitest-short-circuit",
    baseEnv: { VITEST: "1" },
    platform: "linux",
    shellPath: "/bin/sh",
    timeoutMs: 4000,
    behavior: "output",
    output: snapshot([["SHOULD_NOT_RUN", "1"]]),
  },
  {
    name: "sql-null",
    baseEnv: {},
    platform: "linux",
    shellPath: null,
    timeoutMs: 4000,
    behavior: "output",
    output: snapshot([["SHOULD_NOT_RUN", "1"]]),
  },
  {
    name: "executor-throws",
    baseEnv: {},
    platform: "linux",
    shellPath: "/bin/sh",
    timeoutMs: 4000,
    behavior: "throw",
    output: "",
  },
  {
    name: "no-markers",
    baseEnv: {},
    platform: "linux",
    shellPath: "/bin/sh",
    timeoutMs: 4000,
    behavior: "output",
    output: "plain",
  },
  {
    name: "timeout",
    baseEnv: {},
    platform: "linux",
    shellPath: "/bin/sh",
    timeoutMs: 5,
    behavior: "delay",
    delayMs: 50,
    output: snapshot([["TOO_LATE", "1"]]),
  },
];

async function runCaptureCase(testCase: CaptureCase): Promise<unknown> {
  let called = false;
  const result = await captureLoginShellEnvSnapshot({
    baseEnv: testCase.baseEnv as never,
    platform: testCase.platform as never,
    ...(Object.prototype.hasOwnProperty.call(testCase, "shellPath")
      ? { shellPath: testCase.shellPath as never }
      : {}),
    timeoutMs: testCase.timeoutMs,
    executeShell: async () => {
      called = true;
      if (testCase.behavior === "throw") throw new Error("executor failed");
      if (testCase.behavior === "delay") {
        await new Promise((resolve) => setTimeout(resolve, testCase.delayMs ?? 50));
      }
      return testCase.output;
    },
  });
  return {
    captured: result === null ? null : JSON.parse(JSON.stringify(result)),
    called,
  };
}

async function main(): Promise<void> {
  const sandbox = mkdtempSync(path.join(tmpdir(), "mode-runtime-tools-fixtures-"));
  const originalCwd = process.cwd();
  const cases: unknown[] = [];
  try {
    for (const [index, testCase] of CASES.entries()) {
      const root = path.join(sandbox, `case-${index}`);
      mkdirSync(path.join(root, testCase.cwd), { recursive: true });
      materialize(root, testCase.setup);
      const env = withRoot(testCase.env, root);
      process.chdir(path.join(root, testCase.cwd));
      const patch = buildRuntimeToolEnvPatch(testCase.toolIds as never, env as never);
      cases.push({
        name: testCase.name,
        cwd: testCase.cwd,
        toolIds: testCase.toolIds,
        env: testCase.env,
        setup: testCase.setup,
        expected: normalize(JSON.parse(JSON.stringify(patch)), root),
      });
    }
  } finally {
    process.chdir(originalCwd);
    rmSync(sandbox, { recursive: true, force: true });
  }
  const loginSandbox = mkdtempSync(path.join(tmpdir(), "mode-runtime-tools-login-"));
  const login: unknown[] = [];
  try {
    for (const [index, testCase] of LOGIN_CASES.entries()) {
      const root = path.join(loginSandbox, `login-${index}`);
      mkdirSync(root, { recursive: true });
      if (testCase.shellFile) {
        const shell = path.join(root, testCase.shellFile);
        mkdirSync(path.dirname(shell), { recursive: true });
        writeFileSync(shell, "");
      }
      const expected = (await runLoginCase(testCase, root)) as Record<string, unknown>;
      if (testCase.shellPath !== null) {
        const env = withRoot(testCase.baseEnv, root);
        const shell =
          testCase.shellPath !== undefined
            ? testCase.shellPath
            : (env as Record<string, string>).SHELL;
        expected.resolvedShell = shell ? normalize(shell, root) : null;
      }
      login.push({
        name: testCase.name,
        baseEnv: testCase.baseEnv,
        ...(Object.prototype.hasOwnProperty.call(testCase, "shellPath")
          ? { shellPath: testCase.shellPath }
          : {}),
        shellFile: testCase.shellFile ?? null,
        timeoutMs: testCase.timeoutMs ?? null,
        output: testCase.output,
        expected,
      });
    }
  } finally {
    rmSync(loginSandbox, { recursive: true, force: true });
  }
  const agentRuntimeSandbox = mkdtempSync(path.join(tmpdir(), "mode-runtime-agent-"));
  const agentOriginalCwd = process.cwd();
  const agentRuntime: unknown[] = [];
  try {
    for (const [index, testCase] of AGENT_RUNTIME_CASES.entries()) {
      const root = path.join(agentRuntimeSandbox, `agent-${index}`);
      mkdirSync(path.join(root, testCase.cwd), { recursive: true });
      for (const relative of testCase.setup) {
        const target = path.join(root, relative);
        mkdirSync(path.dirname(target), { recursive: true });
        writeFileSync(target, "");
      }
      process.chdir(path.join(root, testCase.cwd));
      agentRuntime.push({
        name: testCase.name,
        cwd: testCase.cwd,
        env: testCase.env,
        setup: testCase.setup,
        expected: runAgentRuntimeCase(testCase, root),
      });
    }
  } finally {
    process.chdir(agentOriginalCwd);
    rmSync(agentRuntimeSandbox, { recursive: true, force: true });
  }
  const bootstrap = [undefined, "/usr/bin:/bin", "/usr/bin:/bin:/usr/bin", ""].map(
    (currentPath) => ({
      currentPath: currentPath ?? null,
      expected: buildShellBootstrapPath(currentPath),
    }),
  );
  const fixture = {
    platform: "win32",
    arch: "x64",
    cases,
    bootstrap,
    login,
    agentRuntime,
    capture: await (async () => {
      const cases: unknown[] = [];
      for (const testCase of CAPTURE_CASES) {
        cases.push({
          name: testCase.name,
          baseEnv: testCase.baseEnv,
          platform: testCase.platform,
          ...(Object.prototype.hasOwnProperty.call(testCase, "shellPath")
            ? { shellPath: testCase.shellPath }
            : {}),
          timeoutMs: testCase.timeoutMs,
          behavior: testCase.behavior,
          delayMs: testCase.delayMs ?? null,
          output: testCase.output,
          expected: await runCaptureCase(testCase),
        });
      }
      return cases;
    })(),
    clientPlatform: [
      { platform: "win32", arch: "x64", env: {} },
      { platform: "darwin", arch: "arm64", env: {} },
      { platform: "linux", arch: "x64", env: {} },
      { platform: "freebsd", arch: "riscv64", env: {} },
      { platform: "win32", arch: "x64", env: { MODE_TEST_RELEASE_TARGET: " linux " } },
      { platform: "win32", arch: "x64", env: { MODE_TEST_RELEASE_ARCH: " arm64 " } },
      { platform: "win32", arch: "x64", env: { MODE_TEST_CLIENT_CONFIG_PLATFORM: " custom " } },
      { platform: "win32", arch: "x64", env: { MODE_TEST_CLIENT_CONFIG_PLATFORM: "   " } },
    ].map((testCase) => {
      const previous = {
        MODE_TEST_RELEASE_TARGET: process.env.MODE_TEST_RELEASE_TARGET,
        MODE_TEST_RELEASE_ARCH: process.env.MODE_TEST_RELEASE_ARCH,
        MODE_TEST_CLIENT_CONFIG_PLATFORM: process.env.MODE_TEST_CLIENT_CONFIG_PLATFORM,
      };
      for (const key of Object.keys(previous) as Array<keyof typeof previous>) {
        if (Object.prototype.hasOwnProperty.call(testCase.env, key)) {
          process.env[key] = testCase.env[key] as string;
        } else {
          delete process.env[key];
        }
      }
      try {
        return {
          ...testCase,
          expected: resolveClientConfigPlatform(
            testCase.platform as never,
            testCase.arch as never,
          ),
        };
      } finally {
        for (const key of Object.keys(previous) as Array<keyof typeof previous>) {
          const value = previous[key];
          if (value === undefined) delete process.env[key];
          else process.env[key] = value;
        }
      }
    }),
  };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${CASES.length} cases)\n`);
}

await main();
