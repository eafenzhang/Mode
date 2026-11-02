/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明终端启动解析矩阵。 */
/**
 * 生成 `terminal-service-resolution` 的 TS 对拍数据。
 *
 * 真源：`packages/services/src/terminal/terminalService.ts` 的
 * `resolveTerminal*With` / `shouldFallbackFromConptyDllWith`。
 *
 * ```bash
 * pnpm terminal-service-resolution:fixtures
 * cargo test -p terminal-service-resolution
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  resolveTerminalCwdWith,
  resolveTerminalEnvWith,
  resolveTerminalShellWith,
  resolveTerminalWindowsPtyInfoWith,
  shouldFallbackFromConptyDllWith,
} from "../packages/services/src/terminal/terminalService.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "terminal-service-resolution",
  "fixtures",
  "resolution.json",
);

const cases = [
  {
    name: "windows-pty",
    kind: "windows-pty",
    platform: "win32",
    release: "10.0.19045",
    env: {},
    cwd: undefined,
    osHome: "C:\\Users\\u",
  },
  {
    name: "linux-pty-empty",
    kind: "windows-pty",
    platform: "linux",
    release: "6.1.0",
    env: {},
    cwd: undefined,
    osHome: "/home/u",
  },
  {
    name: "conpty-errors",
    kind: "conpty",
    errors: [
      "Cannot find conpty.dll",
      "conpty.node module handle is invalid",
      "conpty.node module file name missing",
      "spawn failed error code: 126",
      "error code:  126",
      "shell not found",
      "error code: 127",
    ],
  },
  {
    name: "shell-windows-comspec",
    kind: "shell",
    platform: "win32",
    env: { ComSpec: "C:\\Windows\\cmd.exe" },
    executable: ["C:\\Windows\\cmd.exe"],
  },
  {
    name: "shell-windows-pwsh",
    kind: "shell",
    platform: "win32",
    env: { ComSpec: "C:\\Windows\\cmd.exe" },
    executable: ["pwsh.exe"],
  },
  {
    name: "shell-windows-none",
    kind: "shell",
    platform: "win32",
    env: {},
    executable: [],
  },
  {
    name: "shell-unix-shell-env",
    kind: "shell",
    platform: "linux",
    env: { SHELL: "/bin/fish" },
    executable: ["/bin/fish"],
  },
  {
    name: "shell-unix-fallback",
    kind: "shell",
    platform: "linux",
    env: { SHELL: "/bin/fish" },
    executable: ["/bin/bash"],
  },
  {
    name: "shell-unix-none",
    kind: "shell",
    platform: "linux",
    env: {},
    executable: [],
  },
  {
    name: "cwd-preferred",
    kind: "cwd",
    env: { HOME: "C:\\Users\\u" },
    cwd: "C:\\workspace",
    osHome: "C:\\Users\\u",
    directories: ["C:\\workspace"],
  },
  {
    name: "cwd-home-fallback",
    kind: "cwd",
    env: { HOME: "C:\\Users\\u" },
    cwd: "C:\\missing",
    osHome: "C:\\Users\\u",
    directories: ["C:\\Users\\u"],
  },
  {
    name: "cwd-root-fallback",
    kind: "cwd",
    env: {},
    cwd: "C:\\missing",
    osHome: "C:\\Users\\u",
    directories: ["/"],
  },
  {
    name: "cwd-none",
    kind: "cwd",
    env: {},
    cwd: "C:\\missing",
    osHome: "C:\\Users\\u",
    directories: [],
  },
  {
    name: "env-linux-locale-and-term",
    kind: "env",
    platform: "linux",
    env: {
      PATH: "/usr/bin",
      TERM: "dumb",
      CI: "1",
      COLORTERM: "  ",
      LANG: "C",
      LC_CTYPE: "POSIX",
      LC_ALL: "  ",
      KEEP: "yes",
    },
  },
  {
    name: "env-linux-keeps-utf8",
    kind: "env",
    platform: "linux",
    env: {
      LANG: "zh_CN.UTF-8",
      LC_CTYPE: "en_US.utf8",
      LC_ALL: "C.UTF-8",
      TERM: "xterm",
      COLORTERM: "24bit",
    },
  },
  {
    name: "env-darwin-path-and-locale",
    kind: "env",
    platform: "darwin",
    env: {
      PATH: "/custom:/usr/bin",
      TERM: "xterm-256color",
      COLORTERM: "truecolor",
      LANG: "POSIX",
    },
  },
  {
    name: "env-darwin-no-path",
    kind: "env",
    platform: "darwin",
    env: {},
  },
];

function fixtureCase(testCase: (typeof cases)[number]): Record<string, unknown> {
  const base = {
    name: testCase.name,
    kind: testCase.kind,
    platform: testCase.platform ?? "",
    pathDelimiter: testCase.kind === "env" && testCase.platform === "darwin" ? ":" : testCase.platform === "win32" ? ";" : ":",
    release: testCase.release ?? null,
    env: testCase.env ?? {},
    cwd: testCase.cwd ?? null,
    osHome: testCase.osHome ?? null,
    executable: testCase.executable ?? [],
    directories: testCase.directories ?? [],
    errors: testCase.errors ?? [],
  };
  switch (testCase.kind) {
    case "windows-pty":
      return {
        ...base,
        expected: resolveTerminalWindowsPtyInfoWith(
          testCase.platform as NodeJS.Platform,
          testCase.release!,
        ),
      };
    case "conpty":
      return {
        ...base,
        expected: testCase.errors!.map(shouldFallbackFromConptyDllWith),
      };
    case "shell": {
      try {
        return {
          ...base,
          expected: {
            ok: true,
            value: resolveTerminalShellWith(
              testCase.platform as NodeJS.Platform,
              testCase.env as NodeJS.ProcessEnv,
              (command) => testCase.executable!.includes(command),
            ),
          },
        };
      } catch (error) {
        return { ...base, expected: { ok: false, error: (error as Error).message } };
      }
    }
    case "cwd": {
      try {
        return {
          ...base,
          expected: {
            ok: true,
            value: resolveTerminalCwdWith(
              testCase.cwd,
              testCase.env as NodeJS.ProcessEnv,
              testCase.osHome!,
              (candidate) => testCase.directories!.includes(candidate),
            ),
          },
        };
      } catch (error) {
        return { ...base, expected: { ok: false, error: (error as Error).message } };
      }
    }
    case "env":
      return {
        ...base,
        expected: resolveTerminalEnvWith(
          testCase.platform as NodeJS.Platform,
          testCase.env as NodeJS.ProcessEnv,
          base.pathDelimiter,
        ),
      };
    default:
      throw new Error(`未知 kind: ${testCase.kind}`);
  }
}

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(
  OUTPUT,
  `${JSON.stringify({ cases: cases.map(fixtureCase) }, null, 2)}\n`,
  "utf8",
);
process.stdout.write(`wrote ${OUTPUT}\n`);
