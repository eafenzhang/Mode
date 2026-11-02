/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 Windows shell 组合矩阵。 */
/**
 * 生成 `integrated-terminal-shells` 的 TS 对拍数据。
 *
 * 真源：`packages/services/src/system/integratedTerminalShells.ts`
 *
 * ```bash
 * pnpm integrated-terminal-shells:fixtures
 * cargo test -p integrated-terminal-shells
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { listIntegratedTerminalShellOptions } from "../packages/services/src/system/integratedTerminalShells.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "integrated-terminal-shells",
  "fixtures",
  "shell-options.json",
);

const FIXED_GIT_BASH = "C:\\Program Files\\Git\\bin\\bash.exe";

const cases = [
  { name: "linux-empty", platform: "linux", env: {}, existing: [] },
  { name: "darwin-empty", platform: "darwin", env: {}, existing: [] },
  { name: "windows-default", platform: "win32", env: {}, existing: [] },
  {
    name: "comspec-trim-and-case-insensitive",
    platform: "win32",
    env: { COMSPEC: "  C:\\Windows\\System32\\cmd.exe  ", Path: "", Pathext: "" },
    existing: [],
  },
  {
    name: "fixed-git-bash-system",
    platform: "win32",
    env: { ComSpec: "cmd.exe" },
    existing: [FIXED_GIT_BASH],
  },
  {
    name: "git-from-path-first-inference",
    platform: "win32",
    env: {
      ComSpec: "C:\\Windows\\cmd.exe",
      PATH: "C:\\Tools\\Git\\cmd;C:\\Other",
      PATHEXT: ".EXE;.CMD",
    },
    existing: ["C:\\Tools\\Git\\cmd\\git.exe", "C:\\Tools\\Git\\bin\\bash.exe"],
  },
  {
    name: "git-from-path-second-inference",
    platform: "win32",
    env: {
      PATH: "C:\\Tools\\Git\\usr\\bin",
      PATHEXT: ".EXE",
    },
    existing: ["C:\\Tools\\Git\\usr\\bin\\git.exe", "C:\\Tools\\Git\\bin\\bash.exe"],
  },
  {
    name: "git-exists-but-no-inferred-bash",
    platform: "win32",
    env: { PATH: "C:\\Tools\\Git\\cmd", PATHEXT: ".EXE" },
    existing: ["C:\\Tools\\Git\\cmd\\git.exe"],
  },
  {
    name: "default-pathext-order-finds-cmd",
    platform: "win32",
    env: { PATH: "C:\\Tools;D:\\Bin" },
    existing: ["D:\\Bin\\git.cmd", "C:\\Tools\\git.exe", "C:\\Tools\\git.cmd"],
  },
  {
    name: "path-empty-segments-and-spaces",
    platform: "win32",
    env: {
      ComSpec: "   ",
      Path: ";;C:\\A B\\Git\\cmd; ;C:\\C\\bin;",
      Pathext: " .EXE ;;.CMD ",
    },
    existing: ["C:\\A B\\Git\\cmd\\git.exe", "C:\\A B\\Git\\bin\\bash.exe"],
  },
  {
    name: "fixed-bash-wins-before-path",
    platform: "win32",
    env: { PATH: "C:\\Tools\\Git\\cmd" },
    existing: [FIXED_GIT_BASH, "C:\\Tools\\Git\\cmd\\git.exe", "C:\\Tools\\Git\\bin\\bash.exe"],
  },
];

const fixture = cases.map((testCase) => ({
  name: testCase.name,
  platform: testCase.platform,
  env: testCase.env,
  existing: testCase.existing,
  expected: listIntegratedTerminalShellOptions({
    env: testCase.env as NodeJS.ProcessEnv,
    platform: testCase.platform as NodeJS.Platform,
    isExecutable: (candidate) => testCase.existing.includes(candidate),
  }),
}));

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify({ cases: fixture }, null, 2)}\n`, "utf8");
process.stdout.write(`wrote ${OUTPUT}\n`);
