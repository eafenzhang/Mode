// CUA helper 构建脚本：cargo 产 Windows cdylib → 复制为 cua_ax.node，esbuild 把 helper 入口
// 打成零依赖单文件 entry.cjs，最后按 resolveWindowsCuaRuntime 的 validateRuntimeManifest
// 逐字契约写 runtime-manifest.json（schema/package/platform/arch/electron/entry/addon/sha256）。
// 运行方式：仓库根 `pnpm build:cua-helper`；目标三元组可用 MODE_CUA_AX_TARGET 覆盖（默认 gnu，
// CI 注入 msvc），electron 版本可用 MODE_CUA_ELECTRON_VERSION 覆盖。
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { delimiter, dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

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

const home = process.env.USERPROFILE || process.env.HOME || "";
const llvmMingwBin = join(home, ".rust-tools", "llvm-mingw-20261006-ucrt-x86_64", "bin");
const cargoBin = join(home, ".cargo", "bin");

function fail(message) {
  console.error(`[build-cua-helper] ${message}`);
  process.exit(1);
}

function prependPath(env, dir) {
  const current = (env.PATH || "").split(delimiter).filter(Boolean);
  const same = (a, b) =>
    a.replace(/[/\\]+/gu, "").toLowerCase() === b.replace(/[/\\]+/gu, "").toLowerCase();
  if (existsSync(dir) && !current.some((p) => same(p, dir))) {
    env.PATH = [dir, ...current].join(delimiter);
  }
}

// 本地 gnu 路线的工具链现场装配（progress.md 工具链终案）：llvm-mingw 提供 linker、
// libnode 空桩满足 napi-build 形式要求。目录不存在（如 CI msvc runner）则静默跳过，
// 一律不覆盖调用方已显式设置的环境变量。
function buildCargoEnv() {
  const env = { ...process.env };
  prependPath(env, cargoBin);
  if (target === "x86_64-pc-windows-gnu") {
    prependPath(env, llvmMingwBin);
    if (!env.CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER) {
      const linker = join(llvmMingwBin, "x86_64-w64-mingw32-clang");
      if (existsSync(linker)) env.CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = linker;
    }
    if (!env.LIBNODE_PATH) {
      const stub = join(home, ".rust-tools", "libnode-stub");
      if (existsSync(stub)) env.LIBNODE_PATH = stub;
    }
  }
  return env;
}

// 本机默认 toolchain 的 host 是 msvc，gnu target 的 proc-macro/build 脚本需要 gnu-host
// toolchain；rustup 存在 stable-x86_64-pc-windows-gnu 时用 `+toolchain` 显式选中，
// 没有 rustup 或 toolchain 缺失则退回 plain cargo（由调用方 PATH 决定）。
function cargoArgs(env) {
  const base = ["build", "--release", "--target", target, "--manifest-path", crateManifest];
  if (target !== "x86_64-pc-windows-gnu") return base;
  // 探测必须用装配后的 env：~/.cargo/bin 可能不在调用方 PATH 上，漏掉会静默退回
  // msvc-host toolchain（其 host 产物要链 link.exe，本机无 MSVC 时整条构建炸掉）。
  const probe = spawnSync("rustup", ["toolchain", "list"], { encoding: "utf8", env });
  if (probe.status === 0 && /(^|\n)stable-x86_64-pc-windows-gnu\b/u.test(probe.stdout || "")) {
    return ["+stable-x86_64-pc-windows-gnu", ...base];
  }
  return base;
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

async function resolveElectronVersion() {
  const override = process.env.MODE_CUA_ELECTRON_VERSION?.trim();
  if (override) return override;
  const desktopPkg = JSON.parse(
    await readFile(join(repoRoot, "packages", "desktop", "package.json"), "utf8"),
  );
  const declared = desktopPkg.devDependencies?.electron ?? desktopPkg.dependencies?.electron;
  if (typeof declared !== "string" || !declared.trim()) {
    fail("packages/desktop/package.json 未声明 electron 版本，且 MODE_CUA_ELECTRON_VERSION 未设置");
  }
  // 剥离 ^ ~ >= 等版本范围前缀，manifest 需要纯版本号。
  return declared.trim().replace(/^[\^~>=<\s]+/u, "");
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
    schemaVersion: 1,
    packageName: "@mode/cua",
    packageVersion: modeCuaPkg.version,
    platform: "win32",
    arch: "x64",
    electronVersion: await resolveElectronVersion(),
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
