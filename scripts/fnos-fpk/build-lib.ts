// fnOS fpk 构建库：常量、进程/路径工具、Node 运行时与骨架 staging、fnpack 打包。
// 入口：scripts/build-fnos-fpk.ts；验收：scripts/fnos-fpk/verify.ts。契约见 docs/specs/fnos-fpk-packaging.md。

import { spawnSync, type SpawnSyncOptions } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, createWriteStream, readFileSync } from "node:fs";
import {
  chmod,
  cp,
  mkdir,
  open,
  readdir,
  readFile,
  rename,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { join, resolve } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { createGzip } from "node:zlib";
import assert from "node:assert/strict";

export interface BuildOptions {
  version: string | undefined;
  nodeVersion: string | undefined;
  skipBuild: boolean;
  reuseTarball: boolean;
  verify: boolean;
  smoke: boolean;
  keepWork: boolean;
  help: boolean;
}

export const root = resolve(import.meta.dirname, "..", "..");
export const skeletonDir = resolve(root, "scripts", "fnos-fpk");
export const distRoot = resolve(root, "dist", "fnos");
export const cacheDir = resolve(distRoot, ".cache");
export const workDir = resolve(distRoot, ".work");

export const APPNAME = "mode";
export const DEFAULT_PORT = 3030;
// fnOS 统一网关子路径：ui 入口 url、app.sock 代理前缀、web 构建的 vite base 必须一致。
export const GATEWAY_PREFIX = "/app/mode";
export const WEB_BASE_PATH = `${GATEWAY_PREFIX}/`;
export const DEFAULT_NODE_VERSION = "24.14.0";
export const DEFAULT_NODE_DIST_BASE = "https://cdn.npmmirror.com/binaries/node";
const NODE_DIST_FALLBACK = "https://nodejs.org/dist";
const FNPACK_VERSION = "1.2.3";
const FNPACK_BASE = "https://static2.fnnas.com/fnpack";
// fnpack 1.2.3 发行文件的固定校验值：win32/linux-x64/darwin 为对本仓库 static2.fnnas.com
// 实际下载件计算；官方未发布 linux-arm64 文件（下载 404），该平台缺项时回退为告警，
// 可用 MODE_FNPACK_PATH 指向受信任的本地二进制。
const FNPACK_SHA256: Record<string, string> = {
  "win32-x64": "d7af4bd716b009c58f5bcd931615f39db121e7d4b75dc759e575c4fb2879b6ee",
  "linux-x64": "54b97fa7b70968c4d05c79840f5daeff508957d0bb2062fdb0376d00d9615c93",
  "darwin-x64": "30a9f50a35e8d8d425b687881761478c3c778e9c0da3a1b59f298b666dd7a268",
  "darwin-arm64": "d40cb00896cb2a5d211357d255750ed0cbe7f2d141df671c2b717afb4e74bf77",
};

export function run(command: string, args: string[], options: SpawnSyncOptions = {}): void {
  console.log(`[fnos] ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    ...options,
  });
  if (result.error) {
    throw new Error(`${command} 执行失败：${result.error.message}`, { cause: result.error });
  }
  if (result.status !== 0) {
    throw new Error(`${command} 退出码 ${result.status}`);
  }
}

// Windows 上 GNU tar 会把 `D:\...` 盘符当远程主机（host:path），需要 --force-local（与 build-mode.mjs 同因）；
// 另外实测 msys tar 在 Node 直接 spawn 时对反斜杠形式的 chdir 目标解析不稳定（同一目录正斜杠可用、
// 反斜杠报 ENOENT），统一把参数转正斜杠规避（Windows API 与 msys tar 均接受正斜杠）。
export const tarArgs = (args: string[]): string[] => {
  if (process.platform !== "win32") {
    return args;
  }
  return ["--force-local", ...args.map((arg) => arg.replaceAll("\\", "/"))];
};

export async function pathExists(path: string): Promise<boolean> {
  return Boolean(await stat(path).catch(() => null));
}

export async function isFile(path: string): Promise<boolean> {
  const info = await stat(path).catch(() => null);
  return Boolean(info?.isFile());
}

export async function sha256File(file: string): Promise<string> {
  const hash = createHash("sha256");
  hash.update(await readFile(file));
  return hash.digest("hex");
}

export function normalizeVersion(value: string): string {
  const version = String(value).trim().replace(/^v/u, "");
  if (!/^\d+(\.\d+)*([-+][0-9A-Za-z.-]+)?$/u.test(version)) {
    throw new Error(`版本号不合法（需语义化版本）：${value}`);
  }
  return version;
}

export function resolveVersion(explicit: string | undefined): string {
  if (explicit) {
    return normalizeVersion(explicit);
  }
  const result = spawnSync("git", ["describe", "--tags", "--abbrev=0"], {
    cwd: root,
    encoding: "utf8",
  });
  if (result.status === 0 && result.stdout.trim()) {
    return normalizeVersion(result.stdout.trim());
  }
  const rootPackage = JSON.parse(readFileSync(join(root, "package.json"), "utf8")) as {
    version: string;
  };
  console.warn("[fnos] 无可用 tag，回退 root package.json 版本");
  return normalizeVersion(rootPackage.version);
}

async function download(url: string, destination: string, minBytes: number): Promise<void> {
  const response = await fetch(url);
  if (!response.ok || !response.body) {
    throw new Error(`HTTP ${response.status}`);
  }
  await pipeline(Readable.fromWeb(response.body), createWriteStream(destination));
  const info = await stat(destination).catch(() => null);
  if (!info || info.size < minBytes) {
    throw new Error(`下载内容异常（${info?.size ?? 0} 字节）`);
  }
}

async function ensureNodeArchive(version: string): Promise<string> {
  await mkdir(cacheDir, { recursive: true });
  const file = join(cacheDir, `node-v${version}-linux-x64.tar.gz`);
  if (await isFile(file)) {
    return file;
  }
  const bases = [
    process.env["MODE_NODE_DIST_MIRROR"]?.trim() || DEFAULT_NODE_DIST_BASE,
    NODE_DIST_FALLBACK,
  ];
  for (const base of new Set(bases.map((item) => item.replace(/\/+$/u, "")))) {
    const url = `${base}/v${version}/node-v${version}-linux-x64.tar.gz`;
    try {
      console.log(`[fnos] 下载 Node 运行时：${url}`);
      await download(url, `${file}.partial`, 10 * 1024 * 1024);
      await rename(`${file}.partial`, file);
      return file;
    } catch (error) {
      console.warn(`[fnos] 下载失败（${base}）：${(error as Error).message}`);
      await rm(`${file}.partial`, { force: true });
    }
  }
  throw new Error(`无法下载 Node v${version} linux-x64（可用 MODE_NODE_DIST_MIRROR 指定镜像）`);
}

// 校验目标为 ELF 可执行文件，返回文件大小（用于识别错误页/截断文件等）。
export async function checkElf(file: string, label: string): Promise<number> {
  const info = await stat(file);
  const handle = await open(file, "r");
  try {
    const buffer = Buffer.alloc(4);
    await handle.read(buffer, 0, 4, 0);
    assert.ok(
      buffer[0] === 0x7f && buffer[1] === 0x45 && buffer[2] === 0x4c && buffer[3] === 0x46,
      `${label} 不是 ELF 可执行文件`,
    );
  } finally {
    await handle.close();
  }
  return info.size;
}

// 捆绑 linux-x64 Node 到 app/runtime/bin/node（仅取 bin/node，不带 npm 等发行目录）。
export async function stageNodeBinary(appDir: string, version: string): Promise<void> {
  const archive = await ensureNodeArchive(version);
  const extractDir = join(workDir, "node-extract");
  await rm(extractDir, { recursive: true, force: true });
  await mkdir(extractDir, { recursive: true });
  run(
    "tar",
    tarArgs([
      "-xzf",
      archive,
      "-C",
      extractDir,
      "--strip-components=2",
      `node-v${version}-linux-x64/bin/node`,
    ]),
  );
  const target = join(appDir, "runtime", "bin", "node");
  await mkdir(join(appDir, "runtime", "bin"), { recursive: true });
  await rename(join(extractDir, "node"), target);
  await chmod(target, 0o755).catch(() => {});
  // 官方 Node 二进制仍带 .debug_* 与 .symtab（实测约 5.7MB）。Linux 打包机上 strip
  // 可直接缩小包体；strip 不可用时保留原文件，不影响功能与后续的大小断言。
  if (process.platform === "linux") {
    const stripped = spawnSync("strip", ["--strip-unneeded", target], { stdio: "inherit" });
    if (stripped.error || stripped.status !== 0) {
      console.warn("[fnos] strip 不可用，保留 Node 原始符号表");
    }
  }
  const size = await checkElf(target, "捆绑 Node");
  if (size < 50 * 1024 * 1024) {
    throw new Error(`捆绑 Node 体积异常：${size}`);
  }
  // Node 运行时允许再分发的前提是随附其许可文本：把归档根级 LICENSE 一并放进取 payload。
  const licenseMember = `node-v${version}-linux-x64/LICENSE`;
  const members = await tarLines(archive);
  if (!members.includes(licenseMember)) {
    throw new Error(`Node 归档缺少 ${licenseMember}：无法满足再分发的许可要求`);
  }
  run("tar", tarArgs(["-xzf", archive, "-C", extractDir, "--strip-components=1", licenseMember]));
  await rename(join(extractDir, "LICENSE"), join(appDir, "runtime", "LICENSE"));
}

// 组装 fnpack 项目：manifest/config/cmd/图标来自骨架，app/ 为发行载荷 + ui 入口 + 捆绑 Node。
export async function stageSkeleton(
  pkgDir: string,
  { version, port }: { version: string; port: number },
): Promise<void> {
  const replace = (text: string): string =>
    text.replaceAll("__MODE_VERSION__", version).replaceAll("__MODE_PORT__", String(port));
  await writeFile(
    join(pkgDir, "manifest"),
    replace(await readFile(join(skeletonDir, "manifest.template"), "utf8")),
  );
  await cp(join(skeletonDir, "config"), join(pkgDir, "config"), { recursive: true });
  await cp(join(skeletonDir, "cmd"), join(pkgDir, "cmd"), { recursive: true });
  await cp(join(skeletonDir, "icons", "ICON.PNG"), join(pkgDir, "ICON.PNG"));
  await cp(join(skeletonDir, "icons", "ICON_256.PNG"), join(pkgDir, "ICON_256.PNG"));

  const uiDir = join(pkgDir, "app", "ui");
  await mkdir(join(uiDir, "images"), { recursive: true });
  // app/ui/config 的令牌保留 __MODE_TOKEN__ 占位符：安装/升级/启动时由 cmd/lib.sh 写入真实令牌。
  await writeFile(
    join(uiDir, "config"),
    replace(await readFile(join(skeletonDir, "ui", "config.template"), "utf8")),
  );
  await cp(join(skeletonDir, "ui", "images", "icon_64.png"), join(uiDir, "images", "icon_64.png"));
  await cp(
    join(skeletonDir, "ui", "images", "icon_256.png"),
    join(uiDir, "images", "icon_256.png"),
  );
  // 统一网关接入代理：装在 target 根，由 cmd/main 以捆绑 Node 启动（监听 app.sock）。
  await cp(join(skeletonDir, "proxy.mjs"), join(pkgDir, "app", "proxy.mjs"));

  for (const entry of await readdir(join(pkgDir, "cmd"))) {
    const file = join(pkgDir, "cmd", entry);
    await chmod(file, 0o755).catch(() => {});
    // fnOS 以 bash 直接执行这些脚本；若工作区被 git 检出为 CRLF，脚本会在目标机报 \r 错误，这里 fail-loud。
    const content = await readFile(file, "utf8");
    assert.ok(!content.includes("\r"), `${entry} 含 CRLF 换行（.gitattributes 应保证 LF）`);
  }
}

function fnpackDistSuffix(): string {
  const suffix = {
    "win32-x64": "windows-amd64",
    "linux-x64": "linux-amd64",
    "linux-arm64": "linux-arm64",
    "darwin-x64": "darwin-amd64",
    "darwin-arm64": "darwin-arm64",
  }[`${process.platform}-${process.arch}`];
  if (!suffix) {
    throw new Error(
      `不支持的打包平台：${process.platform}-${process.arch}（可用 MODE_FNPACK_PATH 指定 fnpack 二进制）`,
    );
  }
  return suffix;
}

export async function ensureFnpack(): Promise<string> {
  const fromEnv = process.env["MODE_FNPACK_PATH"]?.trim();
  if (fromEnv) {
    if (!(await isFile(fromEnv))) {
      throw new Error(`MODE_FNPACK_PATH 不存在：${fromEnv}`);
    }
    return fromEnv;
  }
  const platformKey = `${process.platform}-${process.arch}`;
  const baseName = `fnpack-${FNPACK_VERSION}-${fnpackDistSuffix()}`;
  const file = join(cacheDir, process.platform === "win32" ? `${baseName}.exe` : baseName);
  await mkdir(cacheDir, { recursive: true });
  if (!(await isFile(file))) {
    const url = `${FNPACK_BASE}/${baseName}`;
    console.log(`[fnos] 下载 fnpack：${url}`);
    await download(url, `${file}.partial`, 1024 * 1024);
    await rename(`${file}.partial`, file);
  }
  const expected = FNPACK_SHA256[platformKey];
  if (expected) {
    const actual = await sha256File(file);
    if (actual !== expected) {
      throw new Error(`fnpack 校验失败（期望 ${expected}，实际 ${actual}），已拒绝使用`);
    }
  } else {
    console.warn(
      `[fnos] 平台 ${platformKey} 无固定 fnpack 校验值；如需来源可信请用 MODE_FNPACK_PATH 指定本地二进制`,
    );
  }
  await chmod(file, 0o755).catch(() => {});
  const probe = spawnSync(file, ["--help"], { encoding: "utf8" });
  if (probe.status !== 0) {
    throw new Error("fnpack 无法执行；可用 MODE_FNPACK_PATH 指定本地二进制");
  }
  return file;
}

async function gzipFile(source: string, destination: string): Promise<void> {
  await rm(destination, { force: true });
  // 平台自带的 gzip -9 比 Node zlib 的 level 9 更紧凑（同一份 316MB payload 实测
  // 87.61MB vs 89.02MB），所以优先用 CLI；没有 gzip 的环境回退到 Node zlib。
  const handle = await open(destination, "w");
  try {
    const compressed = spawnSync("gzip", ["-9", "-n", "-c", source], {
      stdio: ["ignore", handle.fd, "inherit"],
    });
    if (!compressed.error && compressed.status === 0) {
      return;
    }
    console.warn("[fnos] gzip CLI 不可用，回退到 Node zlib（体积略大）");
  } finally {
    await handle.close();
  }
  await pipeline(
    createReadStream(source),
    createGzip({ level: 9 }),
    createWriteStream(destination),
  );
}

// fnpack build 后重打包：fnpack 在 Windows 不写执行位，必须补 cmd/* 0755（断言失败即失败）。
export async function buildFpk(pkgDir: string, fnpackPath: string, outFile: string): Promise<void> {
  await rm(join(workDir, `${APPNAME}.fpk`), { force: true });
  run(fnpackPath, ["build", "--directory", pkgDir], { cwd: workDir });
  const produced = join(workDir, `${APPNAME}.fpk`);
  if (!(await isFile(produced))) {
    throw new Error(`fnpack 未产出 ${produced}`);
  }

  const repackDir = join(workDir, "repack");
  await rm(repackDir, { recursive: true, force: true });
  await mkdir(repackDir, { recursive: true });
  run("tar", tarArgs(["-xzf", produced, "-C", repackDir]));
  for (const entry of await readdir(join(repackDir, "cmd"))) {
    await chmod(join(repackDir, "cmd", entry), 0o755).catch(() => {});
  }

  // fnpack 用默认级别压缩 app.tgz（实测 89.72MB）。用同一份 payload 以 gzip level 9
  // 重建内层包（实测 87.61MB）：文件清单不变，只是压缩更彻底。入口名沿用 fnpack
  // 的形态（LICENSE、agent 等直接位于包根，没有 `./` 前缀）。
  const appSource = join(pkgDir, "app");
  const appTarFile = join(workDir, `${APPNAME}.app.tar`);
  const appEntries = (await readdir(appSource)).sort();
  // fnpack 输出统一为 0/0 属主；重打包时保持同一口径，避免把构建机用户名写进安装包。
  const ownerArgs = ["--owner=0", "--group=0", "--numeric-owner"];
  await rm(appTarFile, { force: true });
  run("tar", tarArgs(["-cf", appTarFile, ...ownerArgs, "-C", appSource, ...appEntries]));
  await gzipFile(appTarFile, join(repackDir, "app.tgz"));
  await rm(appTarFile, { force: true });

  const entries = (await readdir(repackDir)).sort();
  // win32 下 Node 的 chmod 不落执行位，用 GNU tar --mode 强制（Linux/macOS 用真实 chmod 即可）。
  const modeArgs = process.platform === "win32" ? ["--mode=a+rx"] : [];
  // 先出未压缩 tar，再用 gzip level 9 压缩外层；格式仍是 fnOS 接受的 tar.gz。
  const tarFile = join(workDir, `${APPNAME}.repack.tar`);
  await rm(tarFile, { force: true });
  run("tar", tarArgs(["-cf", tarFile, ...ownerArgs, ...modeArgs, "-C", repackDir, ...entries]));
  await gzipFile(tarFile, outFile);
  await rm(tarFile, { force: true });
}

export async function tarLines(
  archive: string,
  options: { verbose?: boolean } = {},
): Promise<string[]> {
  const result = spawnSync("tar", tarArgs([options.verbose ? "-tvzf" : "-tzf", archive]), {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(`tar 列表失败：${result.stderr}`);
  }
  return result.stdout.split("\n").filter(Boolean);
}
