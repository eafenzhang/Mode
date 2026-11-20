/**
 * 构建 IM SDK 运行时升级包（发布侧，LT-7 T1）。
 *
 * 输入：`packages/services/package.json` 里两个 SDK 的精确版本。默认在临时目录执行
 * `npm install --omit=dev --ignore-scripts` 得到可移植的真实 node_modules，再打包为
 * 确定性 tar.gz + 清单；`--node-modules <dir>` 仅供测试/离线打包，目录必须是真实文件
 * （不接受 pnpm 符号链接树）。
 *
 * 产物（默认 `dist/im-sdk-runtime/`）：
 * - `im-sdk-runtime-<version>.tar.gz`
 * - `im-sdk-runtime-manifest.json`
 *
 * 用法：`pnpm im-sdk-runtime:bundle --version 0.0.142`
 */
import { execFileSync } from "node:child_process";
import { lstat, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

import { createTarGz, sha256Hex, type TarEntry } from "./tar.js";

export const DEFAULT_SDK_PACKAGES = ["@larksuiteoapi/node-sdk", "@wecom/aibot-node-sdk"] as const;
export const MANIFEST_FILE_NAME = "im-sdk-runtime-manifest.json";

const EXCLUDED_SUFFIXES = [".d.ts", ".map", ".tsbuildinfo"];
const EXCLUDED_DIR_NAMES = new Set([".bin"]);
const EXCLUDED_FILE_NAMES = new Set([".package-lock.json"]);
const NATIVE_SUFFIXES = [".node", ".so", ".dylib", ".dll"];
const MAX_INPUT_BYTES = 256 * 1024 * 1024;

export interface SdkPackageSpec {
  name: string;
  version: string;
}

export interface BuildImSdkRuntimeOptions {
  version: string;
  outDir: string;
  /** 跳过 npm install，直接打包给定的 node_modules（测试/离线）。 */
  nodeModulesDir?: string;
  packages?: readonly SdkPackageSpec[];
  createdAt?: string;
}

export interface ImSdkRuntimeBundleManifest {
  schemaVersion: 1;
  version: string;
  createdAt: string;
  hostApiVersion: 1;
  sdkVersions: Record<string, string>;
  entrypoints: Record<string, string>;
  bundle: { file: string; size: number; sha256: string };
  files: Array<{ path: string; size: number; sha256: string }>;
}

export interface BuildImSdkRuntimeResult {
  version: string;
  bundlePath: string;
  manifestPath: string;
  manifest: ImSdkRuntimeBundleManifest;
  fileCount: number;
  bundleBytes: number;
}

export async function buildImSdkRuntimeBundle(
  options: BuildImSdkRuntimeOptions,
): Promise<BuildImSdkRuntimeResult> {
  if (!/^\d+(\.\d+)*$/.test(options.version)) {
    throw new Error(`version must match ^\\d+(\\.\\d+)*$, got "${options.version}"`);
  }
  const packages = options.packages ?? DEFAULT_SDK_PACKAGES.map((name) => ({ name, version: "" }));
  const tempRoot = options.nodeModulesDir ? undefined : await mkdtemp(join(tmpdir(), "im-sdk-"));
  try {
    const nodeModulesDir =
      options.nodeModulesDir ?? (await installProductionTree(tempRoot as string, packages));
    const resolved = await Promise.all(
      packages.map((spec) => resolvePackageEntry(nodeModulesDir, spec)),
    );
    const entries = await collectRuntimeFiles(nodeModulesDir);
    const filePaths = new Set(entries.map((entry) => entry.path));
    for (const packageEntry of resolved) {
      const entryPath = `node_modules/${packageEntry.relativePath}`;
      if (!filePaths.has(entryPath)) {
        throw new Error(`entrypoint missing from runtime files: ${entryPath}`);
      }
    }

    const bundle = createTarGz(entries);
    const manifest: ImSdkRuntimeBundleManifest = {
      schemaVersion: 1,
      version: options.version,
      createdAt: options.createdAt ?? new Date().toISOString(),
      hostApiVersion: 1,
      sdkVersions: Object.fromEntries(resolved.map((item) => [item.name, item.version])),
      entrypoints: Object.fromEntries(
        resolved.map((item) => [item.name, `node_modules/${item.relativePath}`]),
      ),
      bundle: {
        file: `im-sdk-runtime-${options.version}.tar.gz`,
        size: bundle.length,
        sha256: sha256Hex(bundle),
      },
      files: entries.map((entry) => ({
        path: entry.path,
        size: entry.data.length,
        sha256: sha256Hex(entry.data),
      })),
    };

    await mkdir(options.outDir, { recursive: true });
    const bundlePath = join(options.outDir, manifest.bundle.file);
    const manifestPath = join(options.outDir, MANIFEST_FILE_NAME);
    await writeFile(bundlePath, bundle);
    await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
    return {
      version: options.version,
      bundlePath,
      manifestPath,
      manifest,
      fileCount: entries.length,
      bundleBytes: bundle.length,
    };
  } finally {
    if (tempRoot) await rm(tempRoot, { recursive: true, force: true });
  }
}

async function installProductionTree(
  tempRoot: string,
  packages: readonly SdkPackageSpec[],
): Promise<string> {
  const dependencies = Object.fromEntries(
    packages.map((spec) => {
      if (!/^\d+\.\d+\.\d+(?:[-+].+)?$/.test(spec.version)) {
        throw new Error(`SDK ${spec.name} needs an exact version, got "${spec.version}"`);
      }
      return [spec.name, spec.version];
    }),
  );
  await writeFile(
    join(tempRoot, "package.json"),
    `${JSON.stringify({ private: true, dependencies }, null, 2)}\n`,
  );
  const npm = process.platform === "win32" ? "npm.cmd" : "npm";
  execFileSync(
    npm,
    [
      "install",
      "--omit=dev",
      "--ignore-scripts",
      "--no-audit",
      "--no-fund",
      "--package-lock=false",
    ],
    { cwd: tempRoot, stdio: "inherit", shell: process.platform === "win32" },
  );
  return join(tempRoot, "node_modules");
}

async function resolvePackageEntry(
  nodeModulesDir: string,
  spec: SdkPackageSpec,
): Promise<{ name: string; version: string; relativePath: string }> {
  const packageDir = join(nodeModulesDir, ...spec.name.split("/"));
  const packageJsonPath = join(packageDir, "package.json");
  const packageJson = JSON.parse(await readFile(packageJsonPath, "utf8")) as {
    name?: unknown;
    version?: unknown;
    main?: unknown;
    exports?: unknown;
  };
  if (typeof packageJson.version !== "string") {
    throw new Error(`${spec.name} has no version in package.json`);
  }
  if (spec.version && packageJson.version !== spec.version) {
    throw new Error(
      `${spec.name} version drift: expected ${spec.version}, installed ${packageJson.version}`,
    );
  }
  const target =
    resolveExportsEntry(packageJson.exports) ??
    (typeof packageJson.main === "string" ? packageJson.main : "index.js");
  const normalized = target.replace(/^\.\//u, "");
  const absolutePath = resolve(packageDir, normalized);
  const insidePackage = relative(packageDir, absolutePath);
  if (!insidePackage || insidePackage.startsWith("..") || insidePackage.startsWith(sep)) {
    throw new Error(`${spec.name} entrypoint escapes its package: ${target}`);
  }
  const info = await lstat(absolutePath);
  if (!info.isFile()) throw new Error(`${spec.name} entrypoint is not a file: ${target}`);
  return {
    name: spec.name,
    version: packageJson.version,
    relativePath: `${spec.name}/${normalized.split(sep).join("/")}`,
  };
}

function resolveExportsEntry(exportsField: unknown): string | undefined {
  if (exportsField === undefined || exportsField === null) return undefined;
  if (isRecord(exportsField) && Object.keys(exportsField).some((key) => key.startsWith("."))) {
    return resolveExportTarget(exportsField["."]);
  }
  return resolveExportTarget(exportsField);
}

function resolveExportTarget(value: unknown): string | undefined {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) {
    for (const item of value) {
      const target = resolveExportTarget(item);
      if (target) return target;
    }
    return undefined;
  }
  if (isRecord(value)) {
    for (const condition of ["node", "import", "default", "require"]) {
      if (condition in value) {
        const target = resolveExportTarget(value[condition]);
        if (target) return target;
      }
    }
  }
  return undefined;
}

async function collectRuntimeFiles(nodeModulesDir: string): Promise<TarEntry[]> {
  const entries: TarEntry[] = [];
  let totalBytes = 0;
  const walk = async (directory: string): Promise<void> => {
    const dirents = await readdir(directory, { withFileTypes: true });
    for (const dirent of dirents.sort((left, right) => (left.name < right.name ? -1 : 1))) {
      if (EXCLUDED_DIR_NAMES.has(dirent.name) || EXCLUDED_FILE_NAMES.has(dirent.name)) continue;
      const absolutePath = join(directory, dirent.name);
      const info = await lstat(absolutePath);
      if (info.isSymbolicLink()) {
        throw new Error(
          `symlink in runtime tree (${absolutePath}); use npm install or a real file tree`,
        );
      }
      if (info.isDirectory()) {
        await walk(absolutePath);
        continue;
      }
      if (!info.isFile()) continue;
      if (EXCLUDED_SUFFIXES.some((suffix) => dirent.name.endsWith(suffix))) continue;
      if (NATIVE_SUFFIXES.some((suffix) => dirent.name.endsWith(suffix))) {
        throw new Error(
          `native binary in runtime tree (${absolutePath}); the bundle must stay platform-neutral`,
        );
      }
      const data = await readFile(absolutePath);
      totalBytes += data.length;
      if (totalBytes > MAX_INPUT_BYTES) {
        throw new Error(`runtime tree exceeds ${MAX_INPUT_BYTES} bytes`);
      }
      const relativePath = relative(nodeModulesDir, absolutePath).split(sep).join("/");
      entries.push({ path: `node_modules/${relativePath}`, data });
    }
  };
  await walk(nodeModulesDir);
  return entries;
}

async function readServicesVersions(servicesPackagePath: string): Promise<SdkPackageSpec[]> {
  const packageJson = JSON.parse(await readFile(servicesPackagePath, "utf8")) as {
    dependencies?: Record<string, unknown>;
  };
  return DEFAULT_SDK_PACKAGES.map((name) => {
    const version = packageJson.dependencies?.[name];
    if (typeof version !== "string" || !/^\d+\.\d+\.\d+(?:[-+].+)?$/.test(version)) {
      throw new Error(
        `${servicesPackagePath} must pin ${name} to an exact version (got ${String(version)})`,
      );
    }
    return { name, version };
  });
}

function parseArgs(argv: string[]): Map<string, string> {
  const args = new Map<string, string>();
  for (let index = 0; index < argv.length; index += 1) {
    const key = argv[index];
    if (!key?.startsWith("--")) continue;
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) throw new Error(`${key} needs a value`);
    args.set(key.slice(2), value);
    index += 1;
  }
  return args;
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const version = args.get("version") ?? process.env.MODE_IM_SDK_RUNTIME_VERSION;
  if (!version) throw new Error("missing --version (or MODE_IM_SDK_RUNTIME_VERSION)");
  const servicesPackage = args.get("services-package") ?? "packages/services/package.json";
  const packages = await readServicesVersions(resolve(servicesPackage));
  const result = await buildImSdkRuntimeBundle({
    version,
    outDir: resolve(args.get("out") ?? "dist/im-sdk-runtime"),
    ...(args.get("node-modules") ? { nodeModulesDir: resolve(args.get("node-modules")!) } : {}),
    packages,
    ...(args.get("created-at") ? { createdAt: args.get("created-at")! } : {}),
  });
  process.stdout.write(
    `${JSON.stringify(
      {
        version: result.version,
        bundle: result.bundlePath,
        manifest: result.manifestPath,
        fileCount: result.fileCount,
        bundleBytes: result.bundleBytes,
        bundleSha256: result.manifest.bundle.sha256,
        sdkVersions: result.manifest.sdkVersions,
        entrypoints: result.manifest.entrypoints,
      },
      null,
      2,
    )}\n`,
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : "";
if (import.meta.url === invokedPath) {
  main().catch((error: unknown) => {
    process.stderr.write(`[im-sdk-runtime:bundle] ${String(error)}\n`);
    process.exitCode = 1;
  });
}
