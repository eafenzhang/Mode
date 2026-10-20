/**
 * Host sidecar 的**随包发运集**（ship set）与其体积核算（M3 Task 5）。
 *
 * ## 这个模块负责什么
 *
 * `build-host-sidecar.ts` 产出 bundle（`dist-host-sidecar/host-sidecar/index.mjs`），
 * 但它**只有 bundle**：externals 还散在仓库的 `node_modules` 里（hoisted 布局下含传递依赖），
 * 而且没有 Node 运行时。安装包要的是「一个目录，里面什么都有，照原样拷进资源目录就能跑」。
 * 本模块把那个目录造出来，并保证**量到的字节数与拷进去的字节数出自同一个文件清单**：
 *
 * ```
 * dist-host-sidecar/ship/
 *   host-sidecar/index.mjs          ← esbuild 的单文件 bundle
 *   host-sidecar/node_modules/**    ← externals 的传递闭包（平铺布局，见下）
 *   node/node.exe                   ← 随包 Node 运行时
 *   config/provider/mode-builtin.json ← 壳经 MODE_HOST_RESOURCES_DIR 报给 Host 的那份
 * ```
 *
 * 三个落点与壳侧读者一一对应：`host-sidecar/index.mjs` = `mode_tauri::host::resolve_entry`
 * 的资源目录候选；`node/node.exe` = `sidecar_host::pick_node` 的随包档；`config/` = Host 的
 * `resolveBuiltinProviderConfig` 第二档（`MODE_HOST_RESOURCES_DIR`）。
 *
 * ## 平铺布局（为什么不照抄 hoisted 的 `node_modules`）
 *
 * Node 的解析规则是「从文件所在目录逐级向上找 `node_modules`」。把每个闭包包放进
 * `<ship>/host-sidecar/node_modules/<名字>/` 之后，从 `index.mjs` 或任意包内文件出发，
 * 上溯都能命中这同一层——所以平铺是对的，而且**不依赖本 checkout 的 hoisted 布局**
 * （那正是「此刻恰好能解析」的那类依赖面）。
 *
 * 代价写在明处：**同名不同版本的包放不进平铺布局**（会互相覆盖）。所以本模块遇到这种情况
 * **报错退出**并列出冲突名，而不是静默覆盖——静默覆盖出来的包会被装到用户机器上才炸。
 *
 * ## 剪枝规则必须有依据（DR-T5-2）
 *
 * 每条规则都带「为什么可以不发」。剪枝同时报**原始**（未压缩文件字节）与**压缩后**
 * （安装包字节）两个收益：`.node` 二进制的压缩比与文本差一个量级，
 * 只报原始值会让一次 28 MiB 的裁切在门禁上看起来没作用。见 `docs/specs/tauri-renderer-host.md` §13.2。
 */
import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
} from "node:fs";
import { createRequire } from "node:module";
import { basename, dirname, join, relative, sep } from "node:path";

/**
 * 保持 external 的运行时依赖（**逐字**取自 `packages/desktop/tsup.config.ts`：
 * 那批服务在 Electron 宿主上验证过的同一份取舍）。这些包随包带，不进 bundle：
 * `node-pty` 是原生模块，其余几个内部有动态 `require`，内联进 ESM 会在运行时抛
 * `Dynamic require of "x" is not supported`。
 */
export const RUNTIME_EXTERNALS = [
  "node-pty",
  "ssh2",
  "undici",
  "@larksuiteoapi/node-sdk",
  "yaml",
  "node-forge",
  "yauzl",
  "@wecom/aibot-node-sdk",
];

/** 相对仓库根：随包带的那份 Built-in Provider Config（壳经 `MODE_HOST_RESOURCES_DIR` 指过来）。 */
export const BUILTIN_PROVIDER_CONFIG = join("config", "provider", "mode-builtin.json");

/** 三种发运档：`core` = 只有 Host bundle（预算 (a)）；另两档含 Node 运行时与 externals。 */
export type ShipVariant = "core" | "pruned" | "unpruned";

export function formatBytes(bytes: number): string {
  return `${bytes} B (${(bytes / 1024 / 1024).toFixed(2)} MiB)`;
}

/** `relativePath` 是否**就是** `dir` 或在其内部（两种形态都要认，见 `PRUNE_RULES` 的说明）。 */
function isUnder(relativePath: string, dir: string): boolean {
  return relativePath === dir || relativePath.startsWith(`${dir}/`);
}

/** 发运集的目标平台；默认就是构建机，交叉打包时由调用方传入。 */
export interface PruneTarget {
  platform: string;
  arch: string;
}

export const DEFAULT_PRUNE_TARGET: PruneTarget = {
  platform: process.platform,
  arch: process.arch,
};

// 运行期不读取的调试/类型产物：source map、Windows PDB、TypeScript 声明与增量编译缓存。
const DEBUG_ARTIFACT_PATTERN = /(?:\.map|\.pdb|\.tsbuildinfo|\.d\.ts|\.d\.mts|\.d\.cts)$/u;

type PruneRule = (relativePath: string, target: PruneTarget) => boolean;

/**
 * 剪枝规则：`包名 → (包内相对路径，正斜杠，目标平台) => 是否不发`。
 *
 * 规则只写**有依据**的：目标是构建机平台（Windows x64 / Linux x64），
 * 且 DR-T5-2 要求这一条同时报原始与压缩两个收益。
 *
 * **一条实测踩到的坑（两处，都被 `--stage` 的自验与逐包体积量出来）**：
 *
 * 1. 遍历与过滤看到的目录路径**不带尾斜杠**（`prebuilds/win32-x64`），所以判定要写
 *    「等于该目录 **或** 在其下」（[`isUnder`]）；只写 `startsWith("prebuilds/win32-x64/")`
 *    会把那一档**自己**判成要裁的对象。
 * 2. 更隐蔽的一半：**裁掉父目录会连带吞掉它下面要保留的例外**——第一版对
 *    `prebuilds` 本体也返回「裁」，于是 `win32-x64` 从没被判定过，整个 `prebuilds` 消失
 *    （症状：node-pty 只剩 6 MB，比「只留 win32-x64」应有的 ~34 MiB 少了 29 MiB）。
 *    所以父目录一律**保留**，让子项各自被判定。
 */
const PRUNE_RULES: Record<string, PruneRule> = {
  // node-pty 的 `prebuilds/` 是**每个平台**的预编译二进制（Windows 上实测 57.82 MiB）。
  // 只留目标平台的 `prebuilds/{platform}-{arch}`：壳按这个路径解析
  // （`node-pty/lib/utils.js` 的 `loadNativeModule`）。Linux AppImage 之前会连带
  // Windows x64 的整套预编译件，既跑不到又占体积。
  //
  // 非 Windows 目标还要裁掉 Windows 专用的 conpty（OpenConsole.exe / conpty.dll）；
  // `build/Release/pty.node` 是构建机为本平台编译的，目标平台与构建机一致时保留。
  "node-pty": (relativePath, target) => {
    if (relativePath === "prebuilds") return false; // 父目录保留，否则下面的例外无从判定
    if (isUnder(relativePath, "prebuilds")) {
      return !isUnder(relativePath, `prebuilds/${target.platform}-${target.arch}`);
    }
    if (target.platform !== "win32") {
      if (isUnder(relativePath, "third_party/conpty")) return true;
      if (isUnder(relativePath, "build/Release/conpty")) return true;
    }
    if (
      relativePath === "build/Release" &&
      (target.platform !== process.platform || target.arch !== process.arch)
    ) {
      return true;
    }
    return false;
  },
};

/** 剪枝判定的唯一入口（`stageShipSet` 与用例都走它，免得用例测的不是产品走的那条）。 */
export function isPrunedOut(
  packageName: string,
  relativePath: string,
  target: PruneTarget = DEFAULT_PRUNE_TARGET,
): boolean {
  // 纯类型包（某个包把它写进了 `dependencies`）：运行期一次都不会被 require。
  if (packageName.startsWith("@types/")) return true;
  if (DEBUG_ARTIFACT_PATTERN.test(relativePath)) return true;
  return PRUNE_RULES[packageName]?.(relativePath, target) ?? false;
}

/** 一个包在**发运集里**的文件清单（`relativePath` 用正斜杠，跨平台一致）。 */
export interface PackageFile {
  relativePath: string;
  bytes: number;
}

/** 量一棵子树的未压缩文件字节和（**诊断面**：不是门禁口径，门禁看安装包字节）。 */
export function measureTree(root: string): { bytes: number; files: number } {
  // 量「整棵树」时必须**连 `node_modules` 一起量**（发运集的 externals 就在那一层）。
  const files = walkFiles(root, () => false, false);
  return { bytes: files.reduce((total, file) => total + file.bytes, 0), files: files.length };
}

export interface ClosureEntry {
  name: string;
  version: string;
  packageDir: string;
  /** 该包**自己**目录的占用（不含传递依赖、不含嵌套 `node_modules`），未剪枝。 */
  bytes: number;
}

export interface ExternalClosure {
  roots: ClosureEntry[];
  packages: ClosureEntry[];
  /** 解析不到的依赖名（报出来，不静默——静默会让体积数字偏小）。 */
  unresolved: string[];
  /** 同名不同版本的冲突（平铺布局放不下，必须报错）。 */
  collisions: string[];
}

/**
 * 递归求一个目录的占用；`predicate` 返回真时**整个子树**跳过。
 *
 * `skipNestedNodeModules` 是**语境相关**的，两次踩过：
 *
 * - 量**一个包自己**的占用时（`collectExternalClosure`）必须跳过它的嵌套 `node_modules`
 *   ——那一层由闭包遍历单独解析并计数，在这里再算就是双计；
 * - 量**发运集**（或发运集里的 `node_modules`）时必须**不跳过**——发运集的 externals
 *   恰好就在 `host-sidecar/node_modules` 这一层，跳过它会让总量凭空少掉 externals 那一块
 *   （实测：第一版把 135.85 MiB 的发运集报成 93.31 MiB）。
 */
function walkFiles(
  root: string,
  predicate: (relativePath: string) => boolean,
  skipNestedNodeModules = true,
): PackageFile[] {
  const files: PackageFile[] = [];
  const visit = (current: string): void => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      if (skipNestedNodeModules && entry.name === "node_modules") continue;
      const full = join(current, entry.name);
      const relativePath = relative(root, full).split(sep).join("/");
      if (predicate(relativePath)) continue;
      // `statSync` 而不是 Dirent：pnpm 的目录项常是符号链接，跟进去才是真实占用（同 build-host-sidecar）。
      let stat: ReturnType<typeof statSync>;
      try {
        stat = statSync(full);
      } catch {
        continue; // 断链/权限：跳过（闭包侧另有「解析不到」的显式记账）
      }
      if (stat.isDirectory()) visit(full);
      else if (stat.isFile()) files.push({ relativePath, bytes: stat.size });
    }
  };
  visit(root);
  return files;
}

/**
 * 解析 `package.json` 的函数形状——只要「两个参数、返回路径字符串」这一点。
 *
 * **不写 `NodeJS.RequireResolve`**：那个类型是「带 `paths` 属性的调用签名」，而
 * `createRequire(import.meta.url)` 的返回类型是 `NodeJS.Require`（**没有** `paths`），
 * 把它当默认值赋给 `RequireResolve` 是 typecheck 红（TS2741）。调用方真正依赖的只有
 * `resolve(id, { paths })` 这一条，所以按它写形状，两种来源都能赋进来。
 */
export type PackageJsonResolver = (id: string, options: { paths: string[] }) => string;

/**
 * 算 external 的**传递闭包**（按 `package.json` 的 `dependencies`/`optionalDependencies` 逐层解析，
 * 以解析到的 package.json 路径去重）。
 *
 * 为什么不能只加直接 external 的目录：本 checkout 是 hoisted 布局，`@wecom/aibot-node-sdk`
 * 这类包自己的目录里**没有** `node_modules`，它的 `axios`/`ws` 等依赖被提升到别处——
 * 只加外层目录会得到 91.92 MiB 而真实闭包是 102.61 MiB（复评 I2 的实测差额）。
 */
export function collectExternalClosure(
  roots: string[],
  resolvePaths: string[],
  resolve: PackageJsonResolver = createRequire(import.meta.url),
): ExternalClosure {
  const seen = new Map<string, ClosureEntry>();
  const unresolved: string[] = [];
  const duplicates: string[] = [];
  const queue = roots.map((name) => ({ name, from: resolvePaths }));
  while (queue.length > 0) {
    const { name, from } = queue.shift() as { name: string; from: string[] };
    let packageJsonPath: string;
    try {
      packageJsonPath = resolve(`${name}/package.json`, { paths: from });
    } catch {
      unresolved.push(name);
      continue;
    }
    if (seen.has(packageJsonPath)) continue;
    const packageDir = dirname(packageJsonPath);
    const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8")) as {
      version?: unknown;
      dependencies?: Record<string, string>;
      optionalDependencies?: Record<string, string>;
    };
    // 同名不同版本：平铺布局放不下（后拷的会覆盖前面的）。这里只记账，由调用方决定报错与否。
    for (const entry of seen.values()) {
      if (entry.name === name && entry.packageDir !== packageDir) duplicates.push(name);
    }
    seen.set(packageJsonPath, {
      name,
      version: String(packageJson.version ?? "?"),
      packageDir,
      bytes: walkFiles(packageDir, () => false).reduce((total, file) => total + file.bytes, 0),
    });
    const dependencyNames = [
      ...Object.keys(packageJson.dependencies ?? {}),
      ...Object.keys(packageJson.optionalDependencies ?? {}),
    ];
    for (const dependency of dependencyNames) {
      queue.push({ name: dependency, from: [packageDir] });
    }
  }
  const packages = [...seen.values()].sort((left, right) => right.bytes - left.bytes);
  return {
    roots: packages.filter((entry) => roots.includes(entry.name)),
    packages,
    unresolved,
    collisions: [...new Set(duplicates)],
  };
}

export interface ShipSetReport {
  variant: ShipVariant;
  stageRoot: string;
  /** 发运集里**实际**的文件字节和（拷贝完成后重新量的，不是预估）。 */
  bytes: number;
  files: number;
  /** 平铺布局里的包数（core 档为 0）。 */
  packages: number;
  /** 随包 Node 的版本与来源路径（core 档为 null）。 */
  node: { source: string; bytes: number } | null;
}

/** 把 `null` 之外的每一层建出来（`mkdirSync` 的 recursive 会连带建父目录）。 */
function ensureDir(dir: string): void {
  mkdirSync(dir, { recursive: true });
}

/**
 * 造发运集。`variant` 决定带不带 Node 运行时与 externals（见 `ShipVariant`）。
 *
 * **`core` 档也创建空的 `node/` 与 `host-sidecar/node_modules/` 目录**：它们不进安装包
 * （空目录被 Tauri 的 `ResourcePaths` 跳过），存在的理由是 `bundle.resources` 里那三条
 * 路径在三档之间保持不变——否则三档要各改一次配置，而改配置的那一次就是「出的包不是同一份
 * 配置」的来源。
 */
export function stageShipSet(options: {
  variant: ShipVariant;
  bundleFile: string;
  stageRoot: string;
  nodeSource: string;
  builtinProviderConfigSource: string;
  closure: ExternalClosure;
  /**
   * 随包 agent 运行时的**完整 glm 目录**（mode.cjs + 官方插件 + bundled-skills）。
   * 来自 `packages/desktop/bundled-agents/<platform>/glm`。缺省只用于 core 档/单测；
   * 产品档必须提供，否则安装后无法发送任务（2026-10-19 v0.0.57 验收缺陷）。
   */
  agentBundleDir?: string;
  /**
   * Windows CUA helper 运行时（entry.cjs / cua_ax.node / runtime-manifest.json）。
   * 来自 `packages/desktop/bundled-tools/win32-x64/cua-helper`；缺省只用于 core 档/非
   * Windows 目标。产品态解析基准是 `resources/tools/cua-helper`（见 product 分支的
   * `resolvePackagedRuntime`），所以这里落 `tools/cua-helper` 而不是 host-sidecar 内部。
   */
  cuaHelperDir?: string;
}): ShipSetReport {
  const { variant, bundleFile, stageRoot, nodeSource } = options;
  ensureDir(join(stageRoot, "host-sidecar", "node_modules"));
  ensureDir(join(stageRoot, "node"));

  copyFileSync(bundleFile, join(stageRoot, "host-sidecar", "index.mjs"));

  let packages = 0;
  let node: ShipSetReport["node"] = null;
  if (variant !== "core") {
    if (options.closure.collisions.length > 0) {
      throw new Error(
        `平铺布局放不下同名不同版本的包，请先决定版本：${options.closure.collisions.join(", ")}`,
      );
    }
    for (const entry of options.closure.packages) {
      const shouldDrop =
        variant === "pruned"
          ? (relativePath: string) => isPrunedOut(entry.name, relativePath)
          : undefined;
      const target = join(stageRoot, "host-sidecar", "node_modules", entry.name);
      cpSync(entry.packageDir, target, {
        recursive: true,
        // pnpm 的目录项是符号链接：跟进去拷，否则发运集里只有一堆断链。
        dereference: true,
        filter: (source) => {
          const relativePath = relative(entry.packageDir, source).split(sep).join("/");
          if (relativePath === "") return true;
          if (relativePath.split("/").includes("node_modules")) return false;
          return !shouldDrop?.(relativePath);
        },
      });
      packages += 1;
    }
    node = copyNodeRuntime(nodeSource, join(stageRoot, "node"));
  }

  const configTarget = join(stageRoot, BUILTIN_PROVIDER_CONFIG);
  ensureDir(dirname(configTarget));
  copyFileSync(options.builtinProviderConfigSource, configTarget);

  if (options.agentBundleDir) {
    const sourceBundle = join(options.agentBundleDir, "mode.cjs");
    if (!existsSync(sourceBundle)) {
      throw new Error(
        `[host-sidecar-ship] 随包 agent 缺少 ${sourceBundle}；先跑 pnpm --filter @mode/desktop prepare:agent-bundle`,
      );
    }
    const agentTarget = join(stageRoot, "agent", "glm");
    rmSync(agentTarget, { recursive: true, force: true });
    ensureDir(agentTarget);
    cpSync(options.agentBundleDir, agentTarget, {
      recursive: true,
      // pnpm 目录项是符号链接，跟进去拷。
      dereference: true,
      filter: (source) => !basename(source).startsWith(".DS_Store"),
    });
  }

  if (options.cuaHelperDir) {
    // 与 Electron 的 `extraResources: {from: bundled-tools/<platform>/cua-helper, to: tools/cua-helper}`
    // 同一落点：`resolvePackagedRuntime` 只认 `resources/tools/cua-helper`，换目录等于没带。
    const helperTarget = join(stageRoot, "tools", "cua-helper");
    rmSync(helperTarget, { recursive: true, force: true });
    ensureDir(helperTarget);
    cpSync(options.cuaHelperDir, helperTarget, { recursive: true, dereference: true });
  }

  const files = walkFiles(stageRoot, () => false, false);
  return {
    variant,
    stageRoot,
    bytes: files.reduce((total, file) => total + file.bytes, 0),
    files: files.length,
    packages,
    node,
  };
}

/**
 * 拷随包 Node 运行时。**只拷可执行文件本体**：Windows 上的 `node.exe` 静态链接到 UCRT
 * （系统组件），不需要 `nodejs` 安装目录里别的东西。
 *
 * 名字必须是 `node.exe`——`ResourcePaths` 的映射会把文件按原名放进目标目录，
 * 而壳侧候选写死的是 `node/node.exe`（`mode_tauri::host::resolve_config`）。
 * 非 Windows 上留出 `node` 这个名字只是为了脚本本身能跨平台跑（本阶段只出 Windows 包，
 * 见阶段计划的「范围收窄登记」）。
 */
export function copyNodeRuntime(
  nodeSource: string,
  targetDir: string,
): { source: string; bytes: number } {
  const target = join(targetDir, process.platform === "win32" ? "node.exe" : "node");
  copyFileSync(nodeSource, target);
  // Linux 的官方 Node 仍带 .debug_* 与 .symtab（实测约 5.7MB）。AppImage 里这份
  // 运行时会被整包压缩，strip 后的收益按压缩比打折，但仍是净收益；strip 不可用时跳过。
  if (process.platform === "linux") {
    const stripped = spawnSync("strip", ["--strip-unneeded", target], { stdio: "ignore" });
    if (stripped.error || stripped.status !== 0) {
      console.warn("[host-sidecar] strip 不可用，保留随包 Node 原始符号表");
    }
  }
  return { source: nodeSource, bytes: statSync(target).size };
}
