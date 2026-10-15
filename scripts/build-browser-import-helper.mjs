// Windows 浏览器数据导入 helper 的构建脚本：cargo 产 Rust 可执行文件 → 复制到
// packages/desktop/bundled-tools/<platform>-<arch>/browser-import/mode-browser-import-helper.exe
// （宿主解析落点的真源：packages/desktop/src/main/windowsChromeAppBoundKey.ts:119-130）。
//
// 运行方式：`pnpm --filter @mode/desktop prepare:browser-import-helper`。
// 目标三元组可用 MODE_BROWSER_IMPORT_TARGET 覆盖（默认按本机 arch 选 gnu，CI 注入 msvc）。
// 本机 gnu 路线的工具链装配口径与 scripts/build-cua-helper.mjs 一致（llvm-mingw 提供 linker）。
//
// 纪律：本脚本不读取旧 C# 实现，产物行为规格取自 docs/specs/browser-import-helper-contract.md。
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { copyFile, mkdir, readFile } from "node:fs/promises";
import { arch as hostArch, platform as hostPlatform } from "node:process";
import { delimiter, dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const crateManifest = join("crates", "browser-import-helper", "Cargo.toml");
const helperFilename = "mode-browser-import-helper.exe";

const home = process.env.USERPROFILE || process.env.HOME || "";
const llvmMingwBin = join(home, ".rust-tools", "llvm-mingw-20261006-ucrt-x86_64", "bin");
const cargoBin = join(home, ".cargo", "bin");
// gnu-host toolchain：本机默认 toolchain 的 host 是 msvc，gnu target 的 proc-macro/build
// 脚本要链 link.exe（本机无 MSVC）。与 build-cua-helper.mjs 同口径。
const gnuHostToolchain = "stable-x86_64-pc-windows-gnu";

function log(message) {
  console.log(`[browser-import-helper] ${message}`);
}

function fail(message) {
  console.error(`[browser-import-helper] ${message}`);
  process.exit(1);
}

function relFromRepo(path) {
  // 输出只允许仓库相对路径，避免机器绝对路径进日志。
  return relative(repoRoot, path).split(sep).join("/");
}

function archToken() {
  // 与 helper 的 arch_token() 同口径：契约字段是 x64 / arm64，不是 x86_64 / aarch64。
  if (hostArch === "x64") return "x64";
  if (hostArch === "arm64") return "arm64";
  return hostArch;
}

function defaultTarget() {
  if (hostArch === "arm64") return "aarch64-pc-windows-gnu";
  return "x86_64-pc-windows-gnu";
}

function prependPath(env, dir) {
  const current = (env.PATH || "").split(delimiter).filter(Boolean);
  const same = (a, b) =>
    a.replace(/[/\\]+/gu, "").toLowerCase() === b.replace(/[/\\]+/gu, "").toLowerCase();
  if (existsSync(dir) && !current.some((p) => same(p, dir))) {
    env.PATH = [dir, ...current].join(delimiter);
  }
}

// 本机 gnu 路线的工具链现场装配（口径同 build-cua-helper.mjs）：llvm-mingw 提供 linker。
// 目录不存在（如 CI msvc runner）则静默跳过，一律不覆盖调用方已显式设置的环境变量。
function buildCargoEnv(target, appVersion, gitRevision) {
  const env = { ...process.env };
  prependPath(env, cargoBin);
  if (target === "x86_64-pc-windows-gnu" || target === "aarch64-pc-windows-gnu") {
    prependPath(env, llvmMingwBin);
    const linkerVar = `CARGO_TARGET_${target.toUpperCase().replaceAll("-", "_")}_LINKER`;
    if (!env[linkerVar]) {
      const prefix = target.startsWith("aarch64") ? "aarch64-w64-mingw32" : "x86_64-w64-mingw32";
      const linker = join(llvmMingwBin, `${prefix}-clang`);
      if (existsSync(linker)) env[linkerVar] = linker;
    }
  }
  // 契约 A 的第 4/5 字段是构建期注入；cargo 依据 rustc 的 env-dep 记录在取值变化时自动重建。
  env.MODE_APP_VERSION = appVersion;
  env.MODE_GIT_REVISION = gitRevision;
  return env;
}

function cargoArgs(target, env) {
  const base = ["build", "--release", "--target", target, "--manifest-path", crateManifest];
  if (!target.endsWith("-pc-windows-gnu")) return base;
  // 探测必须用装配后的 env：~/.cargo/bin 可能不在调用方 PATH 上，漏掉会静默退回
  // msvc-host toolchain 并因缺 link.exe 整条构建炸掉。
  const probe = spawnSync("rustup", ["toolchain", "list"], { encoding: "utf8", env });
  if (probe.status === 0 && (probe.stdout || "").includes(gnuHostToolchain)) {
    return [`+${gnuHostToolchain}`, ...base];
  }
  return base;
}

function runCargo(target, env) {
  const args = cargoArgs(target, env);
  log(`cargo ${args.join(" ")}`);
  const result = spawnSync("cargo", args, { cwd: repoRoot, env, stdio: "inherit" });
  if (result.error) fail(`无法执行 cargo：${result.error.message}（请确认 cargo 已在 PATH）`);
  if (result.status !== 0) {
    fail(`cargo build 失败（exit ${result.status ?? "signal"}），已中止复制步骤`);
  }
}

// 应用版本的真源是仓库根 package.json（packages/desktop/package.json 无 version 字段）。
// 观测口径：旧产物 --version 第 4 字段为 `0.0.1`，与根清单一致。
async function resolveAppVersion() {
  const pkg = JSON.parse(await readFile(join(repoRoot, "package.json"), "utf8"));
  const version = typeof pkg.version === "string" ? pkg.version.trim() : "";
  if (!version) fail("根 package.json 未声明 version");
  return version;
}

function resolveGitRevision() {
  const result = spawnSync("git", ["rev-parse", "--short", "HEAD"], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  if (result.status !== 0) return "unknown";
  return (result.stdout || "").trim() || "unknown";
}

// 产物自检：契约 A 的 5 字段与冻结的协议名/版本。这里只断言契约本身，
// 不与旧 C# 产物对拍（旧产物发的是 ZCODE_* 品牌，属已修正的缺陷，见契约 spec）。
function verifyBanner(exePath) {
  const result = spawnSync(exePath, ["--version"], { encoding: "utf8" });
  if (result.error) fail(`无法执行产物 ${relFromRepo(exePath)}：${result.error.message}`);
  if (result.status !== 0) fail(`产物 --version 退出码 ${result.status}`);
  const fields = (result.stdout || "").trim().split("\t");
  if (fields.length !== 5) fail(`--version 字段数 ${fields.length}，应为 5：${result.stdout}`);
  if (fields[0] !== "MODE_BROWSER_IMPORT_HELPER") fail(`横幅令牌错误：${fields[0]}`);
  if (fields[1] !== "2") fail(`协议版本错误：${fields[1]}`);
  if (fields[2] !== archToken()) fail(`架构字段 ${fields[2]}，本机期望 ${archToken()}`);
  log(`产物自检通过：${fields.join("\t")}`);
}

async function main() {
  if (hostPlatform !== "win32") {
    // helper 是 Windows 专有的（宿主只在 win32 解析它）；mac/linux CI 腿跳过而非失败。
    log(`skip: 浏览器导入 helper 只在 Windows 构建（当前 ${hostPlatform}）`);
    return;
  }
  const target = process.env.MODE_BROWSER_IMPORT_TARGET || defaultTarget();
  const appVersion = await resolveAppVersion();
  const gitRevision = resolveGitRevision();
  const env = buildCargoEnv(target, appVersion, gitRevision);
  runCargo(target, env);

  const built = join(
    repoRoot,
    "crates",
    "browser-import-helper",
    "target",
    target,
    "release",
    helperFilename,
  );
  if (!existsSync(built)) fail(`cargo 未产出 ${relFromRepo(built)}（target=${target}）`);

  const outputDir = join(repoRoot, "packages", "desktop", "bundled-tools", `win32-${archToken()}`, "browser-import");
  const outputPath = join(outputDir, helperFilename);
  await mkdir(outputDir, { recursive: true });
  await copyFile(built, outputPath);

  verifyBanner(outputPath);
  log(`已产出 ${relFromRepo(outputPath)}（target=${target}，版本 ${appVersion}，修订 ${gitRevision}）`);
}

await main();
