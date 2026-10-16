/**
 * 两份体积预算在**门禁脸**上的实测（M3 Task 5 · DR-T5-1）。
 *
 * ## 这份脚本回答的是哪一个问题
 *
 * 门禁的口径是 **NSIS 安装包字节**（`docs/specs/tauri-platform-commands.md` §8.2 的历史约定，
 * 也是 CI 的 `scripts/tauri-bundle-report.ts` 算的那个数）。而这个数字历史上被三张不同的脸
 * 替代过三次（载荷求和 96.79 MiB / 传递闭包 102.61 MiB / **不含壳 exe** 的未压缩文件字节和
 * 54.41 MiB），于是「这次能不能过门禁」永远无解。本脚本只产出**一张脸**上的数：
 * `statSync(安装包).size`。
 *
 * ## 三档怎么变成两份预算
 *
 * NSIS 把全部载荷压成**一个** LZMA 流，从单一安装包里按文件读不回两半。所以出三个包，
 * 用差值得到 (b)——完整定义在 `docs/specs/tauri-renderer-host.md` §13.2（口径先于数字提交）：
 *
 * | 档 | 包里的资源 | 数字 |
 * | --- | --- | --- |
 * | `core` | Host bundle + `config/` | **预算 (a) 核心** |
 * | `pruned` | 上者 + 随包 Node + **剪枝后**的 externals | 总包；**(b) = 本档 − core 档**（增量） |
 * | `unpruned` | 上者 + 随包 Node + **未剪枝**的 externals | 剪枝的**压缩**收益 = 本档 − pruned 档 |
 *
 * 三档的 `frontendDist` 与壳 exe 完全相同，**唯一的变量是资源目录的内容**，所以差值可解释。
 *
 * ## 为什么本机要走 gnu 路线
 *
 * `tauri build` 自己调 `cargo`，而本机默认 toolchain 的 host 是 msvc（没装 VS Build Tools）。
 * `scripts/rust-cargo.ts` 那条 `+toolchain` 是 cargo 的**参数**，注入不进 tauri-cli，
 * 所以这里改用等价的 `RUSTUP_TOOLCHAIN` 环境变量（toolchain 名从共享探针取，见 `assembleEnv`）。
 *
 * ```bash
 * node --import tsx apps/desktop-tauri/scripts/measure-installer-budgets.ts
 * node --import tsx apps/desktop-tauri/scripts/measure-installer-budgets.ts --variants core,pruned
 * ```
 */
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  cargoHostToolchainArgs,
  defaultGnuTarget,
  withCargoEnv,
} from "../../../scripts/rust-gnu-env.ts";

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, "..");
const repoRoot = resolve(appDir, "../..");
/** 安装包落点（本机不传 `--target` → cargo 用宿主三元组 → `target/release`）。 */
const bundleDir = join(repoRoot, "target", "release", "bundle", "nsis");
/** 采集落点：与 `dist-host-sidecar/` 同一棵被忽略的树，免得三个包散在 `target/` 里不好找。 */
const collectDir = join(appDir, "dist-host-sidecar", "installers");

type Variant = "core" | "pruned" | "unpruned";
const VARIANTS: Variant[] = ["core", "pruned", "unpruned"];

const mib = (bytes: number): string => `${bytes} B (${(bytes / 1024 / 1024).toFixed(2)} MiB)`;

/**
 * 造 `tauri build` 的环境：本机 gnu 路线（CI 的 msvc / Linux 上这几步退化成 no-op）。
 *
 * `RUSTUP_TOOLCHAIN` 的取值来自 `cargoHostToolchainArgs`——那是仓库里**唯一**的 gnu 探针
 * （`scripts/rust-gnu-env.ts` 的文件头要求新入口复用它，不要抄第二份）。它返回
 * `["+stable-x86_64-pc-windows-gnu"]`，去掉 `+` 就是环境变量要的名字。
 */
function assembleEnv(variant: Variant): NodeJS.ProcessEnv {
  const target = defaultGnuTarget();
  const env: NodeJS.ProcessEnv = {
    ...withCargoEnv(process.env, { target }),
    MODE_TAURI_SHIP_VARIANT: variant,
  };
  const toolchain = cargoHostToolchainArgs(env, target)[0]?.replace(/^\+/u, "");
  if (toolchain) env.RUSTUP_TOOLCHAIN = toolchain;
  return env;
}

/** 跑一次 `tauri build --bundles nsis`，返回产出的安装包路径。 */
function buildInstaller(variant: Variant): string {
  // 先清 bundle 目录：不清就会在 `readdirSync` 里捡到上一个档的包（那会让两次测量换尺子）。
  // `maxRetries` 不是保险丝而是必需：刚被 makensis 写完的 .exe 上还挂着句柄时
  // `rm` 会以 `EPERM` 失败（实测踩到一次，整轮测量因此中断）。
  rmSync(bundleDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
  const cli = join(repoRoot, "node_modules", "@tauri-apps", "cli", "tauri.js");
  console.log(`[budgets] tauri build --bundles nsis（发运档 ${variant}）`);
  const result = spawnSync(process.execPath, [cli, "build", "--bundles", "nsis"], {
    cwd: appDir,
    env: assembleEnv(variant),
    stdio: "inherit",
  });
  if (result.status !== 0) {
    throw new Error(`tauri build 失败（档 ${variant}），退出码 ${String(result.status)}`);
  }
  const installers = existsSync(bundleDir)
    ? readdirSync(bundleDir).filter((name) => name.endsWith(".exe"))
    : [];
  if (installers.length !== 1) {
    throw new Error(
      `期望 bundle/nsis 下正好一个 .exe，实际 ${installers.length} 个：${installers.join(", ")}`,
    );
  }
  return join(bundleDir, installers[0] as string);
}

interface Measured {
  variant: Variant;
  path: string;
  bytes: number;
  sha256: string;
}

/** 已采集到的包（`--report-only` 用：不重建，只按门禁脸重算表）。 */
function readCollected(variant: Variant): Measured | null {
  const path = join(collectDir, `Mode-0.0.1-win-x64-${variant}.exe`);
  if (!existsSync(path)) return null;
  const bytes = statSync(path).size;
  return {
    variant,
    path,
    bytes,
    sha256: createHash("sha256").update(readFileSync(path)).digest("hex"),
  };
}

/**
 * 把包收进采集目录（连同 sha256）并量它——**量的是这个文件本身**，门禁脸就是它。
 *
 * **同时快照当时的 `dist/index.html`**（M3 Task 5 实测踩到的坑）：三档各跑一次
 * `tauri build`，`beforeBuildCommand` 每档都重跑一次 Vite 构建，而 renderer 的产物
 * 在**两次构建之间会有几百字节的波动**（`docs/specs/tauri-renderer-host.md` §13.1 已记），
 * 于是**入口 chunk 与那个 CSS chunk 的内容哈希会变**。后果是：拿「最后那次构建的 dist」
 * 去核对「更早那个档的安装包」，会把 2 个名字读成「没被内嵌」——那是**跨批比对**的假红，
 * 不是产物有问题（实测：277 个引用资产里 275 个逐字命中，缺的两个正是入口与 styles）。
 * 快照存在的意义就是让「哪份 index.html 属于这个包」变成可判定的事实，而不是靠跑的顺序。
 */
function collect(variant: Variant, installer: string): Measured {
  mkdirSync(collectDir, { recursive: true });
  // 版本与 `tauri.conf.json` 的占位一致（0.0.1）；档名进文件名，免得三个包互相覆盖。
  const target = join(collectDir, `Mode-0.0.1-win-x64-${variant}.exe`);
  copyFileSync(installer, target);
  const frontendIndex = join(appDir, "dist", "index.html");
  if (existsSync(frontendIndex)) copyFileSync(frontendIndex, `${target}.index.html`);
  const bytes = statSync(target).size;
  const sha256 = createHash("sha256").update(readFileSync(target)).digest("hex");
  return { variant, path: target, bytes, sha256 };
}

function main(): void {
  const argv = process.argv.slice(2);
  const listIndex = argv.indexOf("--variants");
  const selected: Variant[] =
    listIndex >= 0
      ? (argv[listIndex + 1] ?? "")
          .split(",")
          .map((item) => item.trim())
          .filter((item): item is Variant => VARIANTS.includes(item as Variant))
      : VARIANTS;
  if (selected.length === 0)
    throw new Error(`--variants 需要一个非空列表，可选：${VARIANTS.join(" | ")}`);

  const reportOnly = argv.includes("--report-only");
  const measured = reportOnly
    ? selected
        .map((variant) => readCollected(variant))
        .filter((entry): entry is Measured => entry !== null)
    : selected.map((variant) => collect(variant, buildInstaller(variant)));
  if (reportOnly && measured.length !== selected.length) {
    throw new Error("--report-only 要求选中的档都已经采集过（先跑一次不带它的构建）");
  }
  const byVariant = new Map(measured.map((entry) => [entry.variant, entry]));
  const core = byVariant.get("core");
  const pruned = byVariant.get("pruned");
  const unpruned = byVariant.get("unpruned");

  console.log("");
  console.log("[budgets] === 门禁脸：NSIS 安装包字节（statSync 实测，MiB = 字节 / 1024²）===");
  for (const entry of measured) {
    console.log(`[budgets] ${entry.variant.padEnd(9)} ${mib(entry.bytes)}  ${entry.path}`);
    console.log(`[budgets]           sha256 ${entry.sha256}`);
  }
  console.log("");
  if (core) {
    console.log(
      `[budgets] **(a) 核心预算 = ${core.variant} 档的安装包字节 = ${mib(core.bytes)}**` +
        "（壳 exe + UI 资产 + Host bundle + config/；不含 Node 运行时与 externals）",
    );
  }
  if (pruned) {
    console.log(`[budgets] 总包（product 档）= ${mib(pruned.bytes)}`);
  }
  if (core && pruned) {
    console.log(
      `[budgets] **(b) 运行时 + 服务面 = pruned − core = ${mib(pruned.bytes - core.bytes)}**（增量）` +
        "：LZMA 压的是整包，所以这是「加进去以后包变大多少」，不是子集单独压缩后的字节",
    );
  }
  if (pruned && unpruned) {
    console.log(
      `[budgets] **剪枝的压缩收益 = unpruned − pruned = ${mib(unpruned.bytes - pruned.bytes)}**（门禁脸）`,
    );
  }
  if (measured.length !== VARIANTS.length) {
    console.log("[budgets] 只跑了部分档：上面的差值要等缺的档也跑完才算数");
  }
}

main();
