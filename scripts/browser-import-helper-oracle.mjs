// 黑盒对拍取数器：只运行旧产物并记录输入输出，不读也不移植其实现
// （依据 docs/specs/clean-room-policy.md §黑盒对拍）。
// 旧产物属上游衍生实现文件，禁止打开源码；本模块只做进程边界观察。
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

/// 旧 C# 产物的**默认落点**。注意该路径同时是 Rust helper 的产出落点
/// （`scripts/build-browser-import-helper.mjs`）：`prepare:browser-import-helper` 已改指 Rust，
/// 只有 `prepare:browser-import-helper:oracle` 才会把 C# 产物写到这里。
/// 因此对拍旧产物时必须用 `MODE_BROWSER_IMPORT_ORACLE_PATH` 显式指向一份已留存的 C# 副本，
/// 否则读到的会是 Rust 产物。
export const ORACLE_PATH =
  process.env.MODE_BROWSER_IMPORT_ORACLE_PATH?.trim() ||
  join(
    repoRoot,
    "packages",
    "desktop",
    "bundled-tools",
    "win32-x64",
    "browser-import",
    "mode-browser-import-helper.exe",
  );

/// 是否明确指定了旧产物路径。未指定时调用方应跳过「与旧产物对拍」的用例，
/// 不能拿默认路径上当（那里的内容是最后一次构建的结果，不保证是 C#）。
export function hasExplicitOracle() {
  return Boolean(process.env.MODE_BROWSER_IMPORT_ORACLE_PATH?.trim());
}

const RUST_HELPER_FILENAME = "mode-browser-import-helper.exe";

/// Rust 侧产物的候选路径：Windows 本地默认 gnu 交叉目标，CI 走 msvc。
/// 找不到时返回 null，由调用方决定 skip（而不是给假绿）。
export function resolveRustHelperPath() {
  const crateRoot = join(repoRoot, "crates", "browser-import-helper", "target");
  const candidates = [
    join(crateRoot, "x86_64-pc-windows-gnu", "debug", RUST_HELPER_FILENAME),
    join(crateRoot, "x86_64-pc-windows-msvc", "debug", RUST_HELPER_FILENAME),
    join(crateRoot, "x86_64-pc-windows-msvc", "release", RUST_HELPER_FILENAME),
    join(crateRoot, "x86_64-pc-windows-gnu", "release", RUST_HELPER_FILENAME),
  ];
  return candidates.find((candidate) => existsSync(candidate)) ?? null;
}

const DEFAULT_TIMEOUT_MS = 20_000;

function spawnHelper(executablePath, args, stdin, timeoutMs, timeoutMessage) {
  return new Promise((resolve, reject) => {
    const child = spawn(executablePath, args, {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(timeoutMessage));
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

export function runOracle(args, stdin, options = {}) {
  if (!existsSync(ORACLE_PATH)) {
    throw new Error(
      `oracle 缺失：${ORACLE_PATH}\n` +
        "先执行 pnpm --filter @mode/desktop prepare:browser-import-helper:oracle 产出 C# 产物，" +
        "再用 MODE_BROWSER_IMPORT_ORACLE_PATH 指向留存的副本",
    );
  }
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  return spawnHelper(
    ORACLE_PATH,
    args,
    stdin,
    timeoutMs,
    `oracle_timeout: ${args.join(" ")}`,
  );
}

/// 运行 Rust 版 helper；二进制不存在时抛错（调用方应先用 resolveRustHelperPath 判断）。
export function runRustHelper(args, stdin, options = {}) {
  const executablePath = options.executablePath ?? resolveRustHelperPath();
  if (!executablePath) {
    throw new Error("rust helper 未构建：先 cargo build -p browser-import-helper");
  }
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  return spawnHelper(
    executablePath,
    args,
    stdin,
    timeoutMs,
    `rust_helper_timeout: ${args.join(" ")}`,
  );
}

