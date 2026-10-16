// 本机 gnu 路线 Rust 工具链装配的**唯一出处**。
//
// 为什么需要它：开发机上默认 rustup toolchain 的 host 是 msvc，而机器上没装 VS Build
// Tools（且 Git Bash 自带的 coreutils `link` 会挡住 MSVC 的 `link.exe`）。因此本地构建
// 必须走 gnu 路线——rustup 的 `stable-x86_64-pc-windows-gnu` 提供 proc-macro 与 build
// 脚本所需的 gnu 宿主，`~/.rust-tools/llvm-mingw-*` 提供 linker 与 dlltool。
//
// 这段装配原先在 `scripts/build-cua-helper.mjs` 与 `scripts/build-browser-import-helper.mjs`
// 各写了一份，而 `scripts/generate-tauri-protocol.mjs` 要求调用方自己 export PATH/linker。
// 2026-10-16 收敛到这里：新增构建入口直接复用，不要再抄第三份。
//
// 两条贯穿全文件的约束：
//  1. 目录不存在时**静默跳过**——CI 的 msvc / Linux runner 天然没有这些目录，同一份代码
//     必须在那边退化成「什么都不做」；
//  2. **一律不覆盖调用方已显式设置的环境变量**。
//
// 类型注解说明（2026-10-17，M3 Task 5）：本模块原先**没有类型注解**，那时它只被 `.mjs` /
// `.ts` 构建脚本 import（`scripts/` 不在任何 tsc 工程里），所以 typescript 从没看过它。M3
// Task 5 新增的 `measure-installer-budgets.ts` 落在 `apps/desktop-tauri/tsconfig.json` 的
// `include: ["scripts/**/*.ts"]` 里，它 import 本模块 → 本模块第一次进入 tsc 的输入集合，
// 于是「隐式 any」这类问题立刻变成 typecheck 红（release 的门禁）。修法是**在这里补注解**，
// 不是去改调用方的调用形状绕开类型：注解在编译后被完全擦除，**不可能改变运行时行为**，
// 而把调用形状改成「能让类型过」的样子会真的改动行为。
// 函数签名保持不变（同样的参数个数与语义），两个 `.mjs` 消费者不受影响。
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { delimiter, join } from "node:path";

/** 本机 gnu 路线的 rustup 宿主 toolchain。 */
export const GNU_HOST_TOOLCHAIN = "stable-x86_64-pc-windows-gnu";

/** llvm-mingw 的版本目录名。升级工具链时只改这一处。 */
const LLVM_MINGW_DIR = "llvm-mingw-20261006-ucrt-x86_64";

export function homeDir(): string {
  return process.env.USERPROFILE || process.env.HOME || "";
}

/** llvm-mingw 的 bin：提供 `*-w64-mingw32-clang`（linker）与 `dlltool`。 */
export function llvmMingwBinDir(home: string = homeDir()): string {
  return join(home, ".rust-tools", LLVM_MINGW_DIR, "bin");
}

/** cargo 的 bin 目录：调用方的 PATH 里不一定有它（如从 IDE 启动）。 */
export function cargoBinDir(home: string = homeDir()): string {
  return join(home, ".cargo", "bin");
}

/** 是否 gnu 目标三元组（宿主或交叉）。`unknown` 入参是有意的：调用方可能传 `undefined`（无目标）。 */
export function isGnuTarget(target: unknown): target is string {
  return typeof target === "string" && target.endsWith("-pc-windows-gnu");
}

/** loongarch/其他架构目前不走这条路线，只认这两个前缀。 */
function gnuToolPrefix(target: string): string {
  return target.startsWith("aarch64") ? "aarch64-w64-mingw32" : "x86_64-w64-mingw32";
}

/** cargo 的 linker 覆盖变量名：`CARGO_TARGET_<TRIPLE>_LINKER`。 */
export function targetLinkerVar(target: string): string {
  return `CARGO_TARGET_${target.toUpperCase().replaceAll("-", "_")}_LINKER`;
}

/**
 * llvm-mingw 的 clang 包装器路径；本机没有就返回 `null`。
 *
 * **两种文件名都要试**：磁盘上真实存在的是 `x86_64-w64-mingw32-clang.exe`，而 Node 的
 * `existsSync` **不做 PATHEXT 补全**——拿无扩展名的路径去问，在 Windows 上恒为 false。
 * 原先两个构建脚本就只试了无扩展名，于是 linker 变量**从未被设上过**；之所以构建还能过，
 * 是因为 llvm-mingw 的 bin 也进了 PATH，而它自带 `gcc.exe`（rustc 的 gnu 目标默认找
 * `gcc`），等于一直靠 PATH 兜底。这里按「先 `.exe`、再裸名」的顺序命中真实文件，
 * 与 `docs/plans/2026-10-09-tauri-rewrite-roadmap.md` 记的口径（这个 clang 作 linker）
 * 对齐。
 */
function resolveGnuLinker(bin: string, target: string): string | null {
  const bare = join(bin, `${gnuToolPrefix(target)}-clang`);
  return [`${bare}.exe`, bare].find((candidate) => existsSync(candidate)) ?? null;
}

/** 未显式指定时按本机架构选默认 gnu target。 */
export function defaultGnuTarget(arch: string | undefined = process.arch): string {
  return arch === "arm64" ? "aarch64-pc-windows-gnu" : "x86_64-pc-windows-gnu";
}

/**
 * 把 `dir` 置到 PATH 最前（原地修改 `env`）。
 *
 * Windows 上 PATH 的**键名随启动器而变**：Git Bash 导出的是大写 `PATH`，而 cmd/PowerShell
 * 起动 Node 时 `process.env` 里通常是 `Path`。只认大写会让「置首」在后者上退化成静默
 * no-op——`env.PATH = …` 只是新增一个大写键，与原 `Path` 并存，子进程拿到哪一个取决于
 * 环境块的排序。所以两个键名都认，口径与 `scripts/mise-toolchain-env.mjs` 的
 * `withPinnedNodePath` 一致。
 */
export function prependPath(env: NodeJS.ProcessEnv, dir: string): void {
  const key = typeof env.PATH === "string" ? "PATH" : "Path";
  const current = (env[key] ?? "").split(delimiter).filter(Boolean);
  const sameEntry = (a: string, b: string): boolean =>
    a.replace(/[/\\]+/gu, "").toLowerCase() === b.replace(/[/\\]+/gu, "").toLowerCase();
  if (!existsSync(dir) || current.some((entry) => sameEntry(entry, dir))) return;
  env[key] = [dir, ...current].join(delimiter);
}

/**
 * 给 gnu 目标装配 llvm-mingw：把它的 bin 置到 PATH 最前，并指定 linker。
 *
 * 非 gnu 目标、或本机没有 llvm-mingw（CI）时什么都不做。
 */
export function applyGnuToolchain(
  env: NodeJS.ProcessEnv,
  target: string | undefined,
  home: string = homeDir(),
): void {
  if (!isGnuTarget(target)) return;
  const bin = llvmMingwBinDir(home);
  prependPath(env, bin);
  const linkerVar = targetLinkerVar(target);
  if (env[linkerVar]) return;
  const linker = resolveGnuLinker(bin, target);
  if (linker) env[linkerVar] = linker;
}

/**
 * 造一个供 cargo 用的环境：cargo 的 bin 目录 +（gnu 目标时）gnu 工具链。
 *
 * 返回新对象，不改动 `baseEnv`。
 */
export function withCargoEnv(
  baseEnv: NodeJS.ProcessEnv,
  { target, home = homeDir() }: { target?: string | undefined; home?: string } = {},
): NodeJS.ProcessEnv {
  const env = { ...baseEnv };
  prependPath(env, cargoBinDir(home));
  applyGnuToolchain(env, target, home);
  return env;
}

/**
 * 本机存在 gnu 宿主 toolchain 时返回 `["+stable-x86_64-pc-windows-gnu"]`，否则返回 `[]`。
 *
 * 本机默认 toolchain 的 host 是 msvc，而 gnu 目标的 proc-macro / build 脚本要用 gnu 宿主
 * 编译，所以必须用 `+toolchain` 显式选中；没有 rustup 或该 toolchain 缺失时退回 plain
 * cargo，由调用方的 PATH 决定。
 *
 * 探测必须用**装配后**的 `env`：`~/.cargo/bin` 未必在调用方 PATH 上，漏掉会静默退回
 * msvc-host toolchain（其宿主产物要链 `link.exe`，本机无 MSVC 时整条构建炸掉）。
 */
export function cargoHostToolchainArgs(
  env: NodeJS.ProcessEnv,
  target: string | undefined,
): string[] {
  if (!isGnuTarget(target)) return [];
  const probe = spawnSync("rustup", ["toolchain", "list"], { encoding: "utf8", env });
  if (probe.status !== 0) return [];
  const hasGnuHost = new RegExp(`(^|\\n)${GNU_HOST_TOOLCHAIN}\\b`, "u").test(probe.stdout ?? "");
  return hasGnuHost ? [`+${GNU_HOST_TOOLCHAIN}`] : [];
}
