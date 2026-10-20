/**
 * Host sidecar 的打包形态（M3 Task 2 · DR-T2-3；发运集与剪枝归 M3 Task 5）。
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
 * `apps/desktop-tauri/dist-host-sidecar/host-sidecar/index.mjs`，
 * `--stage` 再把 bundle + externals 闭包 + 随包 Node 装成**发运集**（Task 5，
 * 详见 `host-sidecar-ship.ts` 的文件头与 `docs/specs/tauri-renderer-host.md` §13.2）。
 *
 * ## 体积数字必须有出处
 *
 * 脚本跑完打印**实测**字节数（bundle 文件 + 每个 external 包目录的实际占用 + 发运集实测值），
 * 这些数字是体积门禁的输入（DR-T2-3 的代价条款：门禁不达标时改的是打包策略，不是门禁）。
 * **每个数字都标注它测在哪张脸上**：本脚本给的是**未压缩文件字节**（「诊断，不是门禁口径」），
 * 门禁口径是 NSIS 安装包字节，由 `measure-installer-budgets.ts` 出。
 *
 * ## 自验（默认开）
 *
 * 打包产物**必须能真起来**：脚本用 `node <bundle>` 起它（不带 tsx），走一次
 * `host.describe` 的 RPC 往返，断言 `serviceSurface.kind === "services"`。否则「打包形态」
 * 只是一次成功的文件写入——而本仓库反复吃亏的正是「产物生成了，但没人跑过它」。
 *
 * ```bash
 * node --import tsx apps/desktop-tauri/scripts/build-host-sidecar.ts                  # 打包 + 自验 + 报体积
 * node --import tsx apps/desktop-tauri/scripts/build-host-sidecar.ts --no-verify
 * node --import tsx apps/desktop-tauri/scripts/build-host-sidecar.ts --stage pruned   # 造发运集（Task 5）
 * node --import tsx apps/desktop-tauri/scripts/build-host-sidecar.ts --stage core --no-verify
 * ```
 */
import { spawn, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  ChannelClient,
  Emitter,
  VSBuffer,
  type IChannel,
  type IMessagePassingProtocol,
} from "@mode/rpc";
import { build } from "esbuild";

import {
  BUILTIN_PROVIDER_CONFIG,
  RUNTIME_EXTERNALS,
  collectExternalClosure,
  formatBytes,
  measureTree,
  stageShipSet,
  type ShipVariant,
} from "./host-sidecar-ship.ts";

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, "..");
const repoRoot = resolve(appDir, "../..");

/** 入口 = 壳 spawn 的同一个文件（源码形态由 `MODE_HOST_SIDECAR_ENTRY` 指明，见 spec §2）。 */
const ENTRY = resolve(repoRoot, "packages/desktop/src/host-standalone/index.ts");

/** 产物落点：`<outDir>/host-sidecar/index.mjs`（壳侧候选枚举的第一个名字）。 */
const OUT_DIR = resolve(appDir, "dist-host-sidecar");
const OUT_FILE = join(OUT_DIR, "host-sidecar", "index.mjs");
/** 发运集落点（`bundle.resources` 指向它；`dist-host-sidecar/` 已在 `.gitignore` 里）。 */
const STAGE_ROOT = join(OUT_DIR, "ship");

const require_ = createRequire(import.meta.url);

async function bundle(): Promise<number> {
  // 只清**本脚本自己的产物目录**，不清整个 `dist-host-sidecar/`：那里还有发运集与
  // `measure-installer-budgets.ts` 采集安装包用的 `installers/`。第一版清的是整个根目录，
  // 后果是「跑一次构建就抹掉上一个档采集好的安装包」——三档体积表因此在构建之间静默残缺。
  rmSync(join(OUT_DIR, "host-sidecar"), { recursive: true, force: true });
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
    // 产物要能被人读：出问题时至少能看出哪段代码在跑（体积由门禁管，不靠压行）。
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
async function verifyBundle(entryFile: string = OUT_FILE): Promise<string> {
  const sandbox = join(tmpdir(), `mode-host-bundle-${process.pid}`);
  const resources = join(sandbox, "resources");
  mkdirSync(join(resources, "config", "provider"), { recursive: true });
  // 用仓库里那份真配置（不是造一个空文件：服务可能真读它的内容）。
  copyFileSync(join(repoRoot, BUILTIN_PROVIDER_CONFIG), join(resources, BUILTIN_PROVIDER_CONFIG));
  const child = spawn(process.execPath, [entryFile], {
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
      child.stdin.write(
        Buffer.from(bytes.buffer as ArrayBuffer, bytes.byteOffset, bytes.byteLength),
      );
    },
    onMessage: messages.event,
  };
  const client = new ChannelClient(protocol);

  try {
    await new Promise<void>((resolveInit, rejectInit) => {
      const timer = setTimeout(
        () => rejectInit(new Error(`30s 内没有 Initialize；stderr:\n${stderr}`)),
        30_000,
      );
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
      throw new Error(`产物的服务面不是真服务：${JSON.stringify(describe.serviceSurface)}`);
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

/**
 * 解析 `--stage [variant]`；未给出 `--stage` 时返回 null（只打包、不发运）。
 *
 * 档名可以省略：省略时读 `MODE_TAURI_SHIP_VARIANT`，再退回产品默认档 `pruned`。
 * 这样**同一个入口**既能被 CI/人工的 `tauri build`（`beforeBuildCommand` 里的
 * `--stage`，不写档名 → 产品档）用，也能被体积测量脚本用（设环境变量选 unpruned/core）。
 */
function parseStageVariant(argv: string[]): ShipVariant | null {
  const index = argv.indexOf("--stage");
  if (index < 0) return null;
  const next = argv[index + 1];
  const value =
    next !== undefined && !next.startsWith("--")
      ? next
      : process.env.MODE_TAURI_SHIP_VARIANT?.trim() || "pruned";
  if (value !== "core" && value !== "pruned" && value !== "unpruned") {
    throw new Error(`未知的发运档 ${value}（可选：core | pruned | unpruned）`);
  }
  return value;
}

/**
 * 造发运集并报体积。**两组数字都要有**（DR-T5-2）：
 * 剪枝省下的**原始**字节（未压缩文件字节和，诊断面）与剪枝后发运集的实际字节。
 * 「安装包里省了多少」在门禁面上量，由 `measure-installer-budgets.ts` 出。
 *
 * 返回发运集里的入口脚本路径，供调用方**再跑一次**自验（剪枝后的文件集才是会被装到
 * 用户机器上的那一份；只验 bundle 等于没验剪枝）。
 */
/**
 * 随包 agent 的完整 glm 目录。缺失时调用 desktop 的 prepare 脚本构建
 * （mode.cjs + 官方插件 + bundled-skills，与 Electron 包同一份产物）。
 * 发运集必须带上它，否则 Tauri 安装后无法发送任务（v0.0.57 验收缺陷）。
 */
function ensureAgentBundleDir(): string {
  const platformKey = `${process.platform}-${process.arch}`;
  const glmDir = join(repoRoot, "packages/desktop/bundled-agents", platformKey, "glm");
  const required = [
    join(glmDir, "mode.cjs"),
    join(glmDir, "packages", "bundled-skills", "skills", "dynamic-workflows", "SKILL.md"),
  ];
  if (required.every((path) => existsSync(path))) return glmDir;

  console.log("[host-sidecar] 随包 agent 未就绪，执行 prepare:agent-bundle ...");
  const result = spawnSync(
    process.platform === "win32" ? "pnpm.cmd" : "pnpm",
    ["--filter", "@mode/desktop", "prepare:agent-bundle"],
    {
      cwd: repoRoot,
      stdio: "inherit",
      env: process.env,
      shell: process.platform === "win32",
    },
  );
  if (result.status !== 0 || !existsSync(join(glmDir, "mode.cjs"))) {
    throw new Error(
      `[host-sidecar] prepare:agent-bundle 未产出 ${join(glmDir, "mode.cjs")}（exit=${String(result.status)}）`,
    );
  }
  return glmDir;
}

/**
 * 随包 CUA helper 运行时（仅 Windows 产品档）。Tauri 的 Host 是纯 Node sidecar，
 * 没有 Electron 注入的 `process.resourcesPath`；helper 必须随包落到 `tools/cua-helper`，
 * 否则远程桌面 `rd_media_start` 永远解析失败、主控显示「对端不支持远程桌面」。
 *
 * 产物由 `pnpm build:cua-helper`（构建 + stage 到 bundled-tools）产出；这里不自动触发
 * Rust 构建（发布路径已单独执行那一步），缺件时直接报出可执行的命令。
 */
function ensureCuaHelperDir(): string | undefined {
  if (process.platform !== "win32") {
    return undefined;
  }
  const helperDir = join(repoRoot, "packages/desktop/bundled-tools/win32-x64/cua-helper");
  const required = ["entry.cjs", "cua_ax.node", "runtime-manifest.json"];
  if (required.every((name) => existsSync(join(helperDir, name)))) {
    return helperDir;
  }
  throw new Error(
    `[host-sidecar] 随包 CUA helper 未就绪（缺 ${helperDir}/${required.join(",")}）：先运行 pnpm build:cua-helper`,
  );
}

function reportShipSet(variant: ShipVariant): string {
  const closure = collectExternalClosure(RUNTIME_EXTERNALS, [appDir, repoRoot], require_.resolve);
  if (closure.collisions.length > 0) {
    throw new Error(`externals 闭包里有同名不同版本的包：${closure.collisions.join(", ")}`);
  }
  const rawClosureBytes = closure.packages.reduce((total, entry) => total + entry.bytes, 0);
  const nodeSource = process.env.MODE_HOST_NODE_BUNDLE?.trim() || process.execPath;

  rmSync(STAGE_ROOT, { recursive: true, force: true });
  const report = stageShipSet({
    variant,
    bundleFile: OUT_FILE,
    stageRoot: STAGE_ROOT,
    nodeSource,
    builtinProviderConfigSource: join(repoRoot, BUILTIN_PROVIDER_CONFIG),
    closure,
    // core 档只测预算，不带运行时依赖，也不带 agent 与 CUA helper。
    agentBundleDir: variant === "core" ? undefined : ensureAgentBundleDir(),
    cuaHelperDir: variant === "core" ? undefined : ensureCuaHelperDir(),
  });

  console.log(`[host-sidecar] 发运集档 = ${variant}（bundle.resources 指向 ${STAGE_ROOT}）`);
  console.log(
    `[host-sidecar] externals 目录（直接 external，own-dir，**下界**） ${formatBytes(closure.roots.reduce((total, entry) => total + entry.bytes, 0))}`,
  );
  for (const item of closure.roots) {
    console.log(`[host-sidecar]   - ${item.name}@${item.version} ${formatBytes(item.bytes)}`);
  }
  console.log(
    `[host-sidecar] externals 闭包（含传递依赖，去重 ${closure.packages.length} 个包）` +
      ` ${formatBytes(rawClosureBytes)}（**未剪枝**，诊断面）`,
  );
  if (closure.unresolved.length > 0) {
    console.log(`[host-sidecar] ⚠ 解析不到的依赖（数字因此偏小）：${closure.unresolved.join(",")}`);
  }
  if (report.node) {
    console.log(
      `[host-sidecar] 随包 Node ${report.node.source} → node/node.exe ${formatBytes(report.node.bytes)}` +
        `（Node ${process.version}；只拷可执行文件本体）`,
    );
  }
  console.log(
    `[host-sidecar] 发运集实测（**未压缩文件字节和**，诊断，不是门禁口径）：` +
      ` ${formatBytes(report.bytes)} 共 ${report.files} 个文件，平铺 ${report.packages} 个包`,
  );
  const stagedExternals = measureTree(join(STAGE_ROOT, "host-sidecar", "node_modules"));
  console.log(
    `[host-sidecar]   其中 externals（**剪枝后**）： ${formatBytes(stagedExternals.bytes)}` +
      ` 共 ${stagedExternals.files} 个文件`,
  );
  console.log(
    `[host-sidecar] 剪枝**原始**收益 = 未剪枝闭包 − 剪枝后 externals =` +
      ` ${formatBytes(rawClosureBytes - stagedExternals.bytes)}（**诊断，不是门禁口径**；` +
      "安装包里省了多少由 measure-installer-budgets.ts 在门禁面上量）",
  );
  return join(STAGE_ROOT, "host-sidecar", "index.mjs");
}

const verify = !process.argv.includes("--no-verify");

/**
 * 入口（`async main` 而不是顶层 await：本包的 `.ts` 由 tsx 按 CJS 跑，顶层 await 不被支持）。
 */
async function main(): Promise<void> {
  const stageVariant = parseStageVariant(process.argv.slice(2));
  const bundleBytes = await bundle();
  // `--stage` 档必须真跑一次产物自验：发运集是「会被装到用户机器上的那一份」，
  // 它比 bundle 本身更需要被跑过（Task 5 的纪律：剪枝后必须实跑）。
  let verifyResult = "(跳过)";
  if (verify) verifyResult = await verifyBundle();

  console.log("[host-sidecar] 打包形态 = 单文件 ESM bundle + external 的 node_modules 子集");
  console.log(`[host-sidecar] bundle        ${OUT_FILE}`);
  console.log(
    `[host-sidecar] bundle 体积   ${formatBytes(bundleBytes)}（未压缩文件字节和，诊断面）`,
  );
  let stagedVerify = "(跳过)";
  if (stageVariant) {
    const stagedEntry = reportShipSet(stageVariant);
    // `core` 档刻意不发 externals（那是预算 (a) 的口径），所以它跑不起来——不假装跑过。
    if (verify && stageVariant !== "core") stagedVerify = await verifyBundle(stagedEntry);
  }
  console.log(`[host-sidecar] 自验          ${verifyResult}`);
  if (stageVariant)
    console.log(`[host-sidecar] 发运集自验    ${stagedVerify}（平铺布局 + 剪枝后的文件集）`);
}

void main().catch((error: unknown) => {
  console.error(
    `[host-sidecar] 打包或自验失败：${error instanceof Error ? (error.stack ?? error.message) : String(error)}`,
  );
  process.exitCode = 1;
});
