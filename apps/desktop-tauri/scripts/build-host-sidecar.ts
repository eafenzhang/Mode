/**
 * Host sidecar 的打包形态（M3 Task 2 · DR-T2-3）。
 *
 * ## 决定：单文件 ESM bundle + 随包带**外部依赖**的 node_modules 子集
 *
 * 两个候选形态：
 *
 * | 形态 | 做法 | 代价 |
 * | --- | --- | --- |
 * | A **源码树 + node_modules** | 把 `packages/desktop/src/host-standalone` 与它牵到的 workspace 包按源码带过去 | 需要随包带整棵 `packages/*` 源码与 tsx（Node 的 strip-types 不做 `.js`→`.ts` 重映射），体积与解析开销都不可控 |
 * | B **单文件 bundle**（本脚本，采用） | esbuild 把入口与全部 workspace 依赖内联成一个 `index.mjs`，**原生/运行时依赖保持 external** | 需要随包带 external 的那几个包（`node-pty` 等）；`require` 动态加载的包不能内联（同 `packages/desktop/tsup.config.ts` 的 external 清单） |
 *
 * 选 B 的理由不是「更小」，而是**可判定**：bundle 后 sidecar 的运行时依赖面收敛成一个文件 +
 * 一张显式 external 清单；A 的依赖面是「整棵源码树此刻恰好能解析」，出问题时症状是启动期
 * 才暴露的解析错误。external 清单一字不改地取自 Electron 那条已经跑在生产上的清单
 * （`packages/desktop/tsup.config.ts` 的 `desktopNodeRuntimeExternals`）——那是**同一批**服务在
 * 另一条宿主路径上验证过的取舍，不是本任务新发明的。
 *
 * **落点**照 Task 1 壳侧的候选枚举：`<resource>/host-sidecar/index.mjs`
 * （`apps/desktop-tauri/src-tauri/src/host.rs` 的 `resolve_entry`）。本脚本产出
 * `apps/desktop-tauri/dist-host-sidecar/host-sidecar/index.mjs`，Task 3/5 把它放进 bundle 资源目录。
 *
 * ## 体积数字必须有出处
 *
 * 脚本跑完打印**实测**字节数（bundle 文件 + 每个 external 包目录的实际占用），
 * 这些数字是 Task 5 体积门禁的输入（DR-T2-3 的代价条款：门禁不达标时改的是打包策略，
 * 不是门禁）。报告同时写进 `docs/specs/tauri-renderer-host.md` 的证据登记。
 *
 * ## 自验（默认开）
 *
 * 打包产物**必须能真起来**：脚本用 `node <bundle>` 起它（不带 tsx），走一次
 * `host.describe` 的 RPC 往返，断言 `serviceSurface.kind === "services"`。否则「打包形态」
 * 只是一次成功的文件写入——而本仓库反复吃亏的正是「产物生成了，但没人跑过它」。
 *
 * ```bash
 * node --import tsx apps/desktop-tauri/scripts/build-host-sidecar.ts            # 打包 + 自验 + 报体积
 * node --import tsx apps/desktop-tauri/scripts/build-host-sidecar.ts --no-verify
 * ```
 */
import { spawn } from "node:child_process";
import { copyFileSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { ChannelClient, Emitter, VSBuffer, type IChannel, type IMessagePassingProtocol } from "@mode/rpc";
import { build } from "esbuild";

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, "..");
const repoRoot = resolve(appDir, "../..");

/** 入口 = 壳 spawn 的同一个文件（源码形态由 `MODE_HOST_SIDECAR_ENTRY` 指明，见 spec §2）。 */
const ENTRY = resolve(repoRoot, "packages/desktop/src/host-standalone/index.ts");

/** 产物落点：`<outDir>/host-sidecar/index.mjs`（壳侧候选枚举的第一个名字）。 */
const OUT_DIR = resolve(appDir, "dist-host-sidecar");
const OUT_FILE = join(OUT_DIR, "host-sidecar", "index.mjs");

/**
 * 保持 external 的运行时依赖（**逐字**取自 `packages/desktop/tsup.config.ts`：
 * 那批服务在 Electron 宿主上验证过的同一份取舍）。这些包随包带，不进 bundle：
 * `node-pty` 是原生模块，其余几个内部有动态 `require`，内联进 ESM 会在运行时抛
 * `Dynamic require of "x" is not supported`。
 */
const RUNTIME_EXTERNALS = [
  "node-pty",
  "ssh2",
  "undici",
  "@larksuiteoapi/node-sdk",
  "yaml",
  "node-forge",
  "yauzl",
  "@wecom/aibot-node-sdk",
];

const require_ = createRequire(import.meta.url);

function formatBytes(bytes: number): string {
  return `${bytes} B (${(bytes / 1024 / 1024).toFixed(2)} MiB)`;
}

/** 一个包目录的实际占用（递归求和）。**跳过嵌套的 `node_modules`**：那一层的依赖由闭包
 *  遍历单独解析并计数（见 `collectExternalClosure`），在这里再算一遍就是双计。 */
function directorySize(root: string): number {
  let total = 0;
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    if (entry.name === "node_modules") continue;
    const path = join(root, entry.name);
    // `statSync` 而不是 Dirent：pnpm 的目录项常是符号链接，跟进去才是真实占用。
    let stat: ReturnType<typeof statSync>;
    try {
      stat = statSync(path);
    } catch {
      continue; // 断链/权限：跳过（下面另有「解析不到」的显式记账，不静默）
    }
    if (stat.isDirectory()) total += directorySize(path);
    else if (stat.isFile()) total += stat.size;
  }
  return total;
}

interface ExternalClosureEntry {
  name: string;
  version: string;
  /** 该包**自己**目录的占用（不含它的依赖）。 */
  bytes: number;
}

interface ExternalClosure {
  /** 直接 external 的包（清单里的那些）。 */
  roots: ExternalClosureEntry[];
  /** 去重后的全部包（含传递依赖）。 */
  packages: ExternalClosureEntry[];
  /** 直接 external 的 own-dir 之和：**下界**（不含传递依赖）。 */
  ownBytes: number;
  /** 传递闭包去重后的占用：打包形态实际要带的东西。 */
  closureBytes: number;
  /** 解析不到的依赖名（报出来，不静默——静默会让体积数字偏小）。 */
  unresolved: string[];
}

/**
 * 算 external 的**传递闭包**。
 *
 * 为什么不能只加直接 external 的目录：本 checkout 是 hoisted 布局，`@wecom/aibot-node-sdk`
 * 这类包自己的目录里**没有** `node_modules`，它的 `axios`/`ws` 等依赖被提升到别处——
 * 只加外层目录会得到 91.92 MiB 而真实闭包是 102.98 MiB（复评 I2 的实测差额）。
 * 所以按 `package.json` 的 `dependencies`/`optionalDependencies` 逐层解析，
 * 以**解析到的 package.json 路径**去重（不同版本不会互相覆盖），再求和。
 */
function collectExternalClosure(roots: string[]): ExternalClosure {
  const seen = new Map<string, ExternalClosureEntry>();
  const unresolved: string[] = [];
  const queue = roots.map((name) => ({ name, from: [appDir, repoRoot] }));
  while (queue.length > 0) {
    const { name, from } = queue.shift() as { name: string; from: string[] };
    let packageJsonPath: string;
    try {
      packageJsonPath = require_.resolve(`${name}/package.json`, { paths: from });
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
    seen.set(packageJsonPath, {
      name,
      version: String(packageJson.version ?? "?"),
      bytes: directorySize(packageDir),
    });
    // 依赖表合并：两个可选的 `?? {}` 回退是多余写法（lint 规则），取值时再兜底。
    const dependencyNames = [
      ...Object.keys(packageJson.dependencies ?? {}),
      ...Object.keys(packageJson.optionalDependencies ?? {}),
    ];
    for (const dependency of dependencyNames) {
      queue.push({ name: dependency, from: [packageDir] });
    }
  }
  const packages = [...seen.values()].sort((left, right) => right.bytes - left.bytes);
  const sum = (entries: ExternalClosureEntry[]): number =>
    entries.reduce((total, entry) => total + entry.bytes, 0);
  return {
    roots: packages.filter((entry) => roots.includes(entry.name)),
    packages,
    ownBytes: sum(packages.filter((entry) => roots.includes(entry.name))),
    closureBytes: sum(packages),
    unresolved,
  };
}

async function bundle(): Promise<number> {
  rmSync(OUT_DIR, { recursive: true, force: true });
  mkdirSync(dirname(OUT_FILE), { recursive: true });
  await build({
    entryPoints: [ENTRY],
    outfile: OUT_FILE,
    bundle: true,
    platform: "node",
    format: "esm",
    // Node 24（mise.toml 钉的版本；`node:sqlite` 需要较新 Node）。
    target: "node24",
    external: RUNTIME_EXTERNALS,
    sourcemap: false,
    // 产物要能被人读：出问题时至少能看出哪段代码在跑（体积由 Task 5 的门禁管，不靠压行）。
    minify: false,
    legalComments: "none",
    logLevel: "warning",
    // **实测踩到的坑**：内联进来的 CJS 依赖里有**动态** `require("events")` 这类调用，
    // esbuild 的 ESM 产物把它们留成 `__require`，运行时报
    // `Dynamic require of "events" is not supported` —— 而且是在**服务面装配那一步**才炸
    // （存储已经开好了，见 spec §10 的失败现场）。banner 里给产物一个真的 `require`
    // （`createRequire(import.meta.url)`），`__require` 的判定是
    // `typeof require !== "undefined" ? require : 抛错`，所以这一段正好收口。
    banner: {
      js: [
        'import { createRequire as __modeCreateRequire } from "node:module";',
        "const require = __modeCreateRequire(import.meta.url);",
      ].join("\n"),
    },
  });
  return statSync(OUT_FILE).size;
}

/**
 * 走一次真 RPC（`host.describe`）：证明打包产物**跑得起来**，不只是写得出来。
 *
 * **同时覆盖 Built-in Provider Config 的「资源目录」那一档**（复评 M3）：自验刻意
 * **清掉** `MODE_BUILTIN_PROVIDER_CONFIG_FILE`、改设 `MODE_HOST_RESOURCES_DIR` 指向一个临时
 * 资源目录（里面放一份真的 `config/provider/mode-builtin.json`），并断言
 * `builtinProviderConfigSource === "resources"`、`exists === true`。
 * 理由：本机环境变量里往往已经有那个覆写（装了 Mode 的机器就有），若不隔离，自验会在
 * 「env 档」绿灯下报「打包形态 OK」，而**打包形态真正要用的那一档**（资源目录）从没被走过。
 */
async function verifyBundle(): Promise<string> {
  const sandbox = join(tmpdir(), `mode-host-bundle-${process.pid}`);
  const resources = join(sandbox, "resources");
  mkdirSync(join(resources, "config", "provider"), { recursive: true });
  // 用仓库里那份真配置（不是造一个空文件：服务可能真读它的内容）。
  copyFileSync(
    join(repoRoot, "config", "provider", "mode-builtin.json"),
    join(resources, "config", "provider", "mode-builtin.json"),
  );
  const child = spawn(process.execPath, [OUT_FILE], {
    cwd: repoRoot,
    env: {
      ...process.env,
      HOME: sandbox,
      USERPROFILE: sandbox,
      MODE_DATA_BASE_DIR: join(sandbox, "data"),
      MODE_HOST_STARTUP_ID: "bundle-verify",
      MODE_HOST_RESOURCES_DIR: resources,
      // 显式清掉：本机装了 Mode 时它往往已经存在，留着就会把这一档的覆盖变成假绿。
      MODE_BUILTIN_PROVIDER_CONFIG_FILE: "",
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.on("data", (chunk: Buffer) => {
    stderr += chunk.toString();
  });

  // 帧格式：`[4 字节大端长度][payload]`（与壳/桥同一口径）。
  const messages = new Emitter<VSBuffer>();
  let pending = Buffer.alloc(0);
  child.stdout.on("data", (chunk: Buffer) => {
    pending = Buffer.concat([pending, chunk]);
    while (pending.byteLength >= 4) {
      const length = pending.readUInt32BE(0);
      if (pending.byteLength < 4 + length) break;
      messages.fire(VSBuffer.wrap(new Uint8Array(pending.subarray(4, 4 + length))));
      pending = pending.subarray(4 + length);
    }
  });

  const protocol: IMessagePassingProtocol = {
    send: (buffer) => {
      // 按视图边界写：`Buffer` 常是共享池上的视图，直接写 `byteOffset=0` 会把整块池子发出去。
      const bytes = buffer.buffer;
      const header = Buffer.alloc(4);
      header.writeUInt32BE(bytes.byteLength, 0);
      child.stdin.write(header);
      child.stdin.write(Buffer.from(bytes.buffer as ArrayBuffer, bytes.byteOffset, bytes.byteLength));
    },
    onMessage: messages.event,
  };
  const client = new ChannelClient(protocol);

  try {
    await new Promise<void>((resolveInit, rejectInit) => {
      const timer = setTimeout(() => rejectInit(new Error(`30s 内没有 Initialize；stderr:\n${stderr}`)), 30_000);
      client.onDidInitialize(() => {
        clearTimeout(timer);
        resolveInit();
      });
    });
    const describe = await client.getChannel<IChannel>("host").call<{
      name: string;
      serviceSurface?: {
        kind?: string;
        channelCount?: number;
        builtinProviderConfigSource?: string;
        builtinProviderConfigExists?: boolean;
      };
    }>("describe");
    if (describe.serviceSurface?.kind !== "services") {
      throw new Error(`打包产物的服务面不是真服务：${JSON.stringify(describe.serviceSurface)}`);
    }
    // 打包形态真正要用的那一档必须是它（见函数头：不隔离就会被 env 档的绿灯掩盖）。
    if (describe.serviceSurface.builtinProviderConfigSource !== "resources") {
      throw new Error(
        "Built-in Provider Config 未按打包形态（资源目录档）解析，实际来源：" +
          String(describe.serviceSurface.builtinProviderConfigSource),
      );
    }
    if (describe.serviceSurface.builtinProviderConfigExists !== true) {
      throw new Error("资源目录档解析到了路径但文件不存在（打包落点问题）");
    }
    return (
      `${describe.name} channels=${String(describe.serviceSurface.channelCount)}` +
      " builtinConfig=resources(exists=true)"
    );
  } finally {
    // 收摊顺序：先让子进程真的退（Windows 上文件句柄退出前不释放），再删沙箱；
    // 删不掉只留痕——清理失败不该让「打包/自验」的结论变成失败（那会掩盖真正的判定）。
    client.dispose();
    child.stdin.end();
    child.kill();
    await new Promise<void>((resolveExit) => {
      const timer = setTimeout(resolveExit, 5_000);
      child.once("exit", () => {
        clearTimeout(timer);
        resolveExit();
      });
    });
    try {
      rmSync(sandbox, { recursive: true, force: true });
    } catch (error) {
      process.stderr.write(`[host-sidecar] 自验沙箱清理失败（不影响结论）：${String(error)}\n`);
    }
  }
}

const verify = !process.argv.includes("--no-verify");

/**
 * 入口（`async main` 而不是顶层 await：本包的 `.ts` 由 tsx 按 CJS 跑，顶层 await 不被支持）。
 */
async function main(): Promise<void> {
  const bundleBytes = await bundle();
  const closure = collectExternalClosure(RUNTIME_EXTERNALS);
  let verifyResult = "(跳过)";
  if (verify) verifyResult = await verifyBundle();

  console.log("[host-sidecar] 打包形态 = 单文件 ESM bundle + external 的 node_modules 子集");
  console.log(`[host-sidecar] bundle        ${OUT_FILE}`);
  console.log(`[host-sidecar] bundle 体积   ${formatBytes(bundleBytes)}`);
  console.log(
    `[host-sidecar] externals 目录（直接 external，own-dir，**下界**） ${formatBytes(closure.ownBytes)}`,
  );
  for (const item of closure.roots) {
    console.log(`[host-sidecar]   - ${item.name}@${item.version} ${formatBytes(item.bytes)}`);
  }
  console.log(
    `[host-sidecar] externals 闭包（含传递依赖，去重 ${closure.packages.length} 个包）` +
      ` ${formatBytes(closure.closureBytes)}`,
  );
  const closureOnly = closure.packages.filter((entry) => !RUNTIME_EXTERNALS.includes(entry.name));
  for (const item of closureOnly.slice(0, 8)) {
    console.log(`[host-sidecar]   + ${item.name}@${item.version} ${formatBytes(item.bytes)}`);
  }
  if (closureOnly.length > 8) {
    console.log(`[host-sidecar]   + …另有 ${closureOnly.length - 8} 个传递依赖`);
  }
  if (closure.unresolved.length > 0) {
    console.log(`[host-sidecar] ⚠ 解析不到的依赖（数字因此偏小）：${closure.unresolved.join(",")}`);
  }
  console.log(
    `[host-sidecar] 合计（bundle + externals 闭包） ${formatBytes(bundleBytes + closure.closureBytes)}` +
      `（bundle 与 externals 的分账见 spec §13；**不含 Node 运行时本身**）`,
  );
  console.log(`[host-sidecar] 自验          ${verifyResult}`);
}

void main().catch((error: unknown) => {
  console.error(
    `[host-sidecar] 打包或自验失败：${error instanceof Error ? (error.stack ?? error.message) : String(error)}`,
  );
  process.exitCode = 1;
});
