#!/usr/bin/env node
// 构建 fnOS（飞牛）原生应用 fpk：复用 scripts/build-mode.mjs 的发行产物，叠加 fpk 外壳与捆绑 Node 运行时。
// 包结构、状态所有者、验收场景见 docs/specs/fnos-fpk-packaging.md。
//
// 流程：build:mode 发行包 → 解包为 app/ 载荷 → 捆绑 linux-x64 Node → fnpack 打包
//       → 执行位后处理（fnpack 在 Windows 不写执行位）→ 结构断言 / 令牌行为 / 发行冒烟。
//
// 语言政策：本文件为 TypeScript（允许语言），由 Node 24 直接执行（先例 scripts/stage-cua-helper.mts）。

import { mkdir, rename, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

import {
  DEFAULT_NODE_DIST_BASE,
  DEFAULT_NODE_VERSION,
  DEFAULT_PORT,
  WEB_BASE_PATH,
  type BuildOptions,
  buildFpk,
  distRoot,
  ensureFnpack,
  isFile,
  normalizeVersion,
  resolveVersion,
  root,
  run,
  sha256File,
  stageNodeBinary,
  stageSkeleton,
  tarArgs,
  workDir,
} from "./fnos-fpk/build-lib.ts";
import { pruneDistribution } from "./mode-distribution/prune.ts";
import { verifyStructure, verifyTokenFlow } from "./fnos-fpk/verify.ts";

const usage = `Usage:
  node scripts/build-fnos-fpk.ts [options]

构建 fnOS 原生应用（.fpk），产物：dist/fnos/releases/<version>/mode-<version>-x86_64.fpk

Options:
  --version <text>        覆盖版本号（默认取最近 tag，去 v 前缀）
  --node-version <text>   捆绑的 Node 版本（默认 ${DEFAULT_NODE_VERSION}，镜像 ${DEFAULT_NODE_DIST_BASE}）
  --skip-build            透传给 build:mode：复用已有编译产物（仍重新 staging）
  --reuse-tarball         复用 dist/mode/releases 里已存在的发行包，不重新构建
  --no-verify             跳过结构断言 / 令牌行为 / 冒烟（快速迭代用）
  --smoke                 Windows 上强制运行发行冒烟（默认仅 POSIX 运行，见 spec 验收说明）
  --keep-work             保留 dist/fnos/.work 便于排查
  --help, -h              显示本帮助
`;

function parseArgs(argv: string[]): BuildOptions {
  const options: BuildOptions = {
    version: undefined,
    nodeVersion: process.env["MODE_FNOS_NODE_VERSION"]?.trim() || undefined,
    skipBuild: false,
    reuseTarball: false,
    verify: true,
    smoke: false,
    keepWork: false,
    help: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]!;
    const readValue = (): string => {
      if (arg.includes("=")) {
        return arg.slice(arg.indexOf("=") + 1);
      }
      const value = argv[index + 1];
      if (!value || value.startsWith("--")) {
        throw new Error(`缺少参数值：${arg}`);
      }
      index += 1;
      return value;
    };
    if (arg === "--help" || arg === "-h") {
      options.help = true;
    } else if (arg === "--version" || arg.startsWith("--version=")) {
      options.version = readValue();
    } else if (arg === "--node-version" || arg.startsWith("--node-version=")) {
      options.nodeVersion = readValue();
    } else if (arg === "--skip-build") {
      options.skipBuild = true;
    } else if (arg === "--reuse-tarball") {
      options.reuseTarball = true;
    } else if (arg === "--no-verify") {
      options.verify = false;
    } else if (arg === "--smoke") {
      options.smoke = true;
    } else if (arg === "--keep-work") {
      options.keepWork = true;
    } else {
      throw new Error(`未知参数：${arg}\n${usage}`);
    }
  }
  return options;
}

async function main(): Promise<void> {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    console.log(usage);
    return;
  }

  const version = resolveVersion(options.version);
  const nodeVersion = normalizeVersion(options.nodeVersion ?? DEFAULT_NODE_VERSION);
  const port = DEFAULT_PORT;
  const distTarball = resolve(root, "dist", "mode", "releases", version, `mode-${version}.tar.gz`);
  const releaseDir = resolve(distRoot, "releases", version);
  const outFile = resolve(releaseDir, `mode-${version}-x86_64.fpk`);

  console.log(`[fnos] 版本 ${version}；Node ${nodeVersion}；端口 ${port}`);
  await mkdir(releaseDir, { recursive: true });

  if (options.reuseTarball && (await isFile(distTarball))) {
    console.log(`[fnos] 复用发行包：${distTarball}`);
  } else {
    run(
      process.execPath,
      [
        "scripts/build-mode.mjs",
        "--version",
        version,
        "--base-url",
        "https://example.invalid/mode/",
        ...(options.skipBuild ? ["--skip-build"] : []),
      ],
      // web 以网关子路径为 base 构建：资产/WS/API 全部跟随 /app/mode/（网关形态必需）。
      { env: { ...process.env, MODE_WEB_BASE: WEB_BASE_PATH } },
    );
  }
  if (!(await isFile(distTarball))) {
    throw new Error(`缺少发行包：${distTarball}`);
  }

  await rm(workDir, { recursive: true, force: true });
  const extractedDir = join(workDir, "extracted");
  const pkgDir = join(workDir, "pkg");
  const verifyDir = join(workDir, "verify-app");
  await mkdir(extractedDir, { recursive: true });
  await mkdir(pkgDir, { recursive: true });
  run("tar", tarArgs(["-xzf", distTarball, "-C", extractedDir]));
  await rename(join(extractedDir, "mode"), join(pkgDir, "app"));

  // fnOS 只运行 linux-x64：把 build:mode 为多平台保留的其他原生件裁掉。
  // 通用调试文件裁剪也在这里兜底执行，--reuse-tarball 复用旧包时同样生效。
  const pruned = await pruneDistribution(join(pkgDir, "app"), {
    targetPlatform: "linux",
    targetArch: "x64",
  });
  console.log(
    `[fnos] 目标平台裁剪：${pruned.files} 个文件，` +
      `${(pruned.bytes / 1024 / 1024).toFixed(1)} MiB（未压缩，含 ${pruned.directories} 个目录）`,
  );

  await stageNodeBinary(join(pkgDir, "app"), nodeVersion);
  await stageSkeleton(pkgDir, { version, port });

  const fnpackPath = await ensureFnpack();
  await buildFpk(pkgDir, fnpackPath, outFile);

  if (options.verify) {
    await verifyStructure({ fpkFile: outFile, verifyDir, version, port });
    await verifyTokenFlow(verifyDir);
    if (process.platform === "win32" && !options.smoke) {
      console.log(
        "[fnos] 跳过发行冒烟（Windows 无交互控制台时 TUI/conpty 环节不可用；--smoke 可强制）。",
      );
      console.log(
        `[fnos] 发布级验证请在 Linux/macOS 执行：node scripts/mode-distribution-smoke.mjs ${distTarball}`,
      );
    } else {
      run(process.execPath, ["scripts/mode-distribution-smoke.mjs", distTarball]);
    }
  } else {
    console.log("[fnos] --no-verify：跳过结构断言 / 令牌行为 / 冒烟");
  }

  const sha256 = await sha256File(outFile);
  await writeFile(`${outFile}.sha256`, `${sha256}  ${outFile.split(/[\\/]/u).at(-1)}\n`);
  console.log("");
  console.log(`[fnos] fpk:    ${outFile}`);
  console.log(`[fnos] sha256: ${sha256}`);
  console.log(`[fnos] 安装：飞牛 fnOS → 应用中心 → 手动安装 → 选择该 .fpk`);

  if (!options.keepWork) {
    await rm(workDir, { recursive: true, force: true });
  } else {
    console.log(`[fnos] 已保留工作目录：${workDir}`);
  }
}

await main();
