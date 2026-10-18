// CUA helper 构建脚本：cargo 产 Windows cdylib → 复制为 cua_ax.node，esbuild 把 helper 入口
// 打成零依赖单文件 entry.cjs，最后按 resolveWindowsCuaRuntime 的 validateRuntimeManifest
// 逐字契约写 runtime-manifest.json（schema/package/platform/arch/napi/entry/addon/sha256）。
// 运行方式：仓库根 `pnpm build:cua-helper`；目标三元组可用 MODE_CUA_AX_TARGET 覆盖（默认 gnu，
// CI 注入 msvc）。
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { cargoHostToolchainArgs, homeDir, withCargoEnv } from "./rust-gnu-env.ts";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const target = process.env.MODE_CUA_AX_TARGET || "x86_64-pc-windows-gnu";
const crateManifest = join("crates", "mode-cua-ax", "Cargo.toml");
const distDir = join(repoRoot, "packages", "mode-cua", "dist-cua-helper");
const entrySource = join(repoRoot, "packages", "mode-cua", "helper", "entry.mjs");
const addonDll = join(
  repoRoot,
  "crates",
  "mode-cua-ax",
  "target",
  target,
  "release",
  "mode_cua_ax.dll",
);
// manifest 的 entry/addon 是相对 runtime 根的规范路径（validateRuntimeManifest 校验字段值本身，
// 不含目录前缀）；modeCuaRuntime 契约则带 dist-cua-helper/ 前缀，两者不可混写。
const entryName = "entry.cjs";
const addonName = "cua_ax.node";
const manifestName = "runtime-manifest.json";
// 与 crates/mode-cua-ax/Cargo.toml 的 napi features 对齐：默认 napi3 + 追加 napi4，
// 即插件实际要求的最低 N-API 版本是 4。改 crate features 时必须同步这里。
const REQUIRED_NAPI_VERSION = 4;

function fail(message) {
  console.error(`[build-cua-helper] ${message}`);
  process.exit(1);
}

// gnu 工具链的装配口径（llvm-mingw 的 PATH 与 linker 变量）在 scripts/rust-gnu-env.ts，
// 那是唯一出处；本函数只补 CUA 专有的 libnode 空桩（napi-build 的形式要求）。
function buildCargoEnv() {
  const env = withCargoEnv(process.env, { target });
  if (target === "x86_64-pc-windows-gnu" && !env.LIBNODE_PATH) {
    const stub = join(homeDir(), ".rust-tools", "libnode-stub");
    if (existsSync(stub)) env.LIBNODE_PATH = stub;
  }
  return env;
}

function cargoArgs(env) {
  const base = ["build", "--release", "--target", target, "--manifest-path", crateManifest];
  return [...cargoHostToolchainArgs(env, target), ...base];
}

function runCargo(env) {
  const args = cargoArgs(env);
  console.log(`[build-cua-helper] cargo ${args.join(" ")}`);
  const result = spawnSync("cargo", args, { cwd: repoRoot, env, stdio: "inherit" });
  if (result.error) fail(`无法执行 cargo：${result.error.message}（请确认 cargo 已在 PATH）`);
  if (result.status !== 0) {
    fail(`cargo build 失败（exit ${result.status ?? "signal"}），已中止后续打包步骤`);
  }
}

async function sha256File(path) {
  // 与 resolveWindowsCuaRuntime 的 defaultHashBytes 同口径：sha256(文件原始字节)。
  return createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
}

async function bundleEntry() {
  let esbuild;
  try {
    // esbuild 经 tsup 依赖已在仓库根 node_modules；build-time 专用，mode-cua 保持零运行时依赖。
    esbuild = createRequire(join(repoRoot, "package.json"))("esbuild");
  } catch (error) {
    fail(`无法解析 esbuild（先执行 pnpm install）：${error?.message ?? String(error)}`);
  }
  try {
    await esbuild.build({
      entryPoints: [entrySource],
      outfile: join(distDir, entryName),
      bundle: true,
      platform: "node",
      format: "cjs",
      target: "node22",
      // CJS 产物里 esbuild 会把 import.meta 清空（只发警告），而 helper/addon.mjs 的
      // createRequire(import.meta.url) 会因此拿到 undefined 并在装载 addon 时炸掉；
      // 把 import.meta.url 定义为 __filename（entry.cjs 的绝对路径）即可让 createRequire 正常工作。
      define: { "import.meta.url": "__filename" },
      logLevel: "silent",
    });
  } catch (error) {
    const details = (error?.errors ?? []).map((e) => e.text).join("; ");
    fail(`esbuild 打包失败：${details || error?.message || String(error)}`);
  }
}

function relFromRepo(path) {
  // 输出只允许仓库相对路径，避免机器绝对路径进日志。
  return relative(repoRoot, path).split(sep).join("/");
}

async function main() {
  const env = buildCargoEnv();
  runCargo(env);
  if (!existsSync(addonDll)) {
    fail(`cargo 未产出 ${relFromRepo(addonDll)}（target=${target}）`);
  }
  await mkdir(distDir, { recursive: true });
  await copyFile(addonDll, join(distDir, addonName));
  await bundleEntry();

  const modeCuaPkg = JSON.parse(
    await readFile(join(repoRoot, "packages", "mode-cua", "package.json"), "utf8"),
  );
  const manifest = {
    schemaVersion: 2,
    packageName: "@mode/cua",
    packageVersion: modeCuaPkg.version,
    platform: "win32",
    arch: "x64",
    napiVersion: REQUIRED_NAPI_VERSION,
    entry: entryName,
    addon: addonName,
    sha256: {
      entry: await sha256File(join(distDir, entryName)),
      addon: await sha256File(join(distDir, addonName)),
    },
  };
  const manifestPath = join(distDir, manifestName);
  await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

  console.log(`[build-cua-helper] 产物（target=${target}）：`);
  for (const file of [entryName, addonName, manifestName]) {
    console.log(`  ${relFromRepo(join(distDir, file))}`);
  }
  console.log(`  sha256 ${entryName}: ${manifest.sha256.entry}`);
  console.log(`  sha256 ${addonName}: ${manifest.sha256.addon}`);
  console.log(`[build-cua-helper] runtime-manifest.json：\n${JSON.stringify(manifest, null, 2)}`);
}

await main();
