// 黑盒对拍取数器：只运行旧产物并记录输入输出，不读也不移植其实现
// （依据 docs/specs/clean-room-policy.md §黑盒对拍）。
// 旧产物属上游衍生实现文件，禁止打开源码；本模块只做进程边界观察。
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

export const ORACLE_PATH = join(
  repoRoot,
  "packages",
  "desktop",
  "bundled-tools",
  "win32-x64",
  "browser-import",
  "mode-browser-import-helper.exe",
);

const DEFAULT_TIMEOUT_MS = 20_000;

export function runOracle(args, stdin, options = {}) {
  if (!existsSync(ORACLE_PATH)) {
    throw new Error(
      `oracle 缺失：${ORACLE_PATH}\n先执行 pnpm --filter @mode/desktop prepare:browser-import-helper`,
    );
  }
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  return new Promise((resolve, reject) => {
    const child = spawn(ORACLE_PATH, args, {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`oracle_timeout: ${args.join(" ")}`));
    }, timeoutMs);
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", (exitCode) => {
      clearTimeout(timer);
      resolve({ exitCode, stdout, stderr });
    });
    child.stdin.end(stdin ?? "");
  });
}
