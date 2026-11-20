/**
 * IM SDK 运行时定位与加载（在线升级的读取侧）。
 *
 * 优先级：`<appConfigDir>/im-sdk-runtime/versions/<version>`（active 指针，校验通过）
 * → 随包 `node_modules`（返回 undefined 由调用方回退）。任何一处校验失败都整体回退，
 * 不加载半套升级产物；裁决与安全边界见 `docs/specs/im-sdk-runtime.md`。
 */
import { readFileSync, statSync } from "node:fs";
import { isAbsolute, join, relative, resolve } from "node:path";
import { readExternalEnvVar } from "@mode/shared";

import { createServiceLogger } from "../logger/serviceLogger.js";
import { getAppConfigDir } from "../paths.js";
import {
  IM_SDK_RUNTIME_MANIFEST_FILE_NAME,
  isValidImSdkRuntimeVersion,
  isRecord,
  parseImSdkRuntimeManifest,
  type ImSdkRuntimeManifest,
} from "./imSdkManifest.js";

export const IM_SDK_RUNTIME_DIR_NAME = "im-sdk-runtime";
export const IM_SDK_RUNTIME_MANIFEST_URL_ENV = "MODE_IM_SDK_RUNTIME_MANIFEST_URL";
export const IM_SDK_RUNTIME_PUBKEY_ENV = "MODE_IM_SDK_RUNTIME_PUBKEY";
export const IM_SDK_RUNTIME_DISABLE_ENV = "MODE_IM_SDK_RUNTIME_DISABLE";

const logger = createServiceLogger("bots:im-sdk-runtime");
const warned = new Set<string>();

export interface ImSdkRuntimeScope {
  root?: string;
  env?: NodeJS.ProcessEnv;
}

export interface ImSdkRuntimeLayout {
  root: string;
  activeFile: string;
  versionsDir: string;
}

export interface ImSdkRuntimePointer {
  schemaVersion: 1;
  version: string;
  activatedAt: string;
  bundleSha256?: string;
}

export interface ActiveImSdkRuntime {
  version: string;
  directory: string;
  manifest: ImSdkRuntimeManifest;
  manifestBytes: Buffer;
  bundleSha256?: string;
}

export function resolveImSdkRuntimeRoot(scope: ImSdkRuntimeScope = {}): string {
  const configured = scope.root?.trim();
  return configured ? resolve(configured) : join(getAppConfigDir(), IM_SDK_RUNTIME_DIR_NAME);
}

export function resolveImSdkRuntimeLayout(scope: ImSdkRuntimeScope = {}): ImSdkRuntimeLayout {
  const root = resolveImSdkRuntimeRoot(scope);
  return {
    root,
    activeFile: join(root, "active.json"),
    versionsDir: join(root, "versions"),
  };
}

export function isImSdkRuntimeDisabled(env: NodeJS.ProcessEnv = process.env): boolean {
  const value = env[IM_SDK_RUNTIME_DISABLE_ENV]?.trim().toLowerCase();
  return value === "1" || value === "true" || value === "yes";
}

/** 只解析 `active.json`，不验证版本目录是否存在（升级器做降级判断要用原始指针）。 */
export function readImSdkRuntimePointer(
  scope: ImSdkRuntimeScope = {},
): ImSdkRuntimePointer | undefined {
  const { activeFile } = resolveImSdkRuntimeLayout(scope);
  let text: string;
  try {
    text = readFileSync(activeFile, "utf8");
  } catch (error) {
    if (isFileMissing(error)) return undefined;
    warnOnce(`pointer-read:${activeFile}`, `cannot read IM SDK runtime pointer: ${message(error)}`);
    return undefined;
  }
  try {
    const raw: unknown = JSON.parse(text);
    if (!isRecord(raw) || raw.schemaVersion !== 1) {
      throw new Error("schemaVersion is not 1");
    }
    if (typeof raw.version !== "string" || !isValidImSdkRuntimeVersion(raw.version)) {
      throw new Error("version is missing or malformed");
    }
    if (typeof raw.activatedAt !== "string" || raw.activatedAt.trim() === "") {
      throw new Error("activatedAt is missing");
    }
    return {
      schemaVersion: 1,
      version: raw.version,
      activatedAt: raw.activatedAt,
      ...(typeof raw.bundleSha256 === "string" ? { bundleSha256: raw.bundleSha256 } : {}),
    };
  } catch (error) {
    warnOnce(
      `pointer-invalid:${activeFile}`,
      `ignoring invalid IM SDK runtime pointer (${message(error)}); using bundled SDKs`,
    );
    return undefined;
  }
}

/**
 * 解析当前生效的升级运行时。指针、清单、入口三点任一不成立都返回 undefined，
 * 调用方回退随包 SDK——可用的旧版本优于半套升级产物。
 */
export function readActiveImSdkRuntime(
  scope: ImSdkRuntimeScope = {},
): ActiveImSdkRuntime | undefined {
  if (isImSdkRuntimeDisabled(scope.env ?? process.env)) return undefined;
  const pointer = readImSdkRuntimePointer(scope);
  if (!pointer) return undefined;
  const { versionsDir } = resolveImSdkRuntimeLayout(scope);
  const directory = join(versionsDir, pointer.version);
  try {
    const manifestBytes = readFileSync(join(directory, IM_SDK_RUNTIME_MANIFEST_FILE_NAME));
    const manifest = parseImSdkRuntimeManifest(manifestBytes);
    if (manifest.version !== pointer.version) {
      throw new Error(
        `manifest version ${manifest.version} does not match active pointer ${pointer.version}`,
      );
    }
    for (const [packageName, entrypoint] of Object.entries(manifest.entrypoints)) {
      const entryPath = resolve(directory, entrypoint);
      if (!statSync(entryPath).isFile()) {
        throw new Error(`entrypoint for ${packageName} is not a file`);
      }
    }
    return {
      version: pointer.version,
      directory,
      manifest,
      manifestBytes,
      ...(pointer.bundleSha256 ? { bundleSha256: pointer.bundleSha256 } : {}),
    };
  } catch (error) {
    warnOnce(
      `active-invalid:${directory}`,
      `ignoring IM SDK runtime ${pointer.version} (${message(error)}); using bundled SDKs`,
    );
    return undefined;
  }
}

/**
 * 解析某个 SDK 包的**入口文件**绝对路径。undefined = 使用随包 `node_modules`。
 *
 * 清单里的 entrypoint 是发布侧固化的入口文件（不是 package.json）：避免
 * `require`/`import` 条件分支在客户端得到不同入口。解析结果必须落在运行时目录内，
 * 防止清单被篡改后指向宿主其它 node_modules。
 */
export function resolveImSdkEntry(
  packageName: string,
  scope: ImSdkRuntimeScope = {},
): string | undefined {
  const active = readActiveImSdkRuntime(scope);
  if (!active) return undefined;
  const entrypoint = active.manifest.entrypoints[packageName];
  if (!entrypoint) {
    warnOnce(
      `missing-entrypoint:${active.version}:${packageName}`,
      `IM SDK runtime ${active.version} has no entrypoint for ${packageName}; using bundled SDK`,
    );
    return undefined;
  }
  try {
    const entryPath = resolve(active.directory, entrypoint);
    const relativePath = relative(active.directory, entryPath);
    if (!relativePath || relativePath.startsWith("..") || isAbsolute(relativePath)) {
      throw new Error("entrypoint is outside the runtime directory");
    }
    if (!statSync(entryPath).isFile()) throw new Error("entrypoint is not a file");
    return entryPath;
  } catch (error) {
    warnOnce(
      `entry-resolve:${active.version}:${packageName}`,
      `cannot resolve ${packageName} from IM SDK runtime ${active.version} (${message(error)}); using bundled SDK`,
    );
    return undefined;
  }
}

/** 激活/回退后清空告警去重与解析缓存，让下一次通道启动重新判定。 */
export function invalidateImSdkRuntimeCache(): void {
  warned.clear();
}

export function describeImSdkRuntime(scope: ImSdkRuntimeScope = {}): {
  mode: "bundled" | "upgraded";
  version?: string;
  root: string;
} {
  const active = readActiveImSdkRuntime(scope);
  const root = resolveImSdkRuntimeRoot(scope);
  return active ? { mode: "upgraded", version: active.version, root } : { mode: "bundled", root };
}

export function readImSdkRuntimeManifestUrl(
  env: NodeJS.ProcessEnv = process.env,
): string | undefined {
  return readExternalEnvVar(env, IM_SDK_RUNTIME_MANIFEST_URL_ENV);
}

export function readImSdkRuntimePublicKey(
  env: NodeJS.ProcessEnv = process.env,
): string | undefined {
  return readExternalEnvVar(env, IM_SDK_RUNTIME_PUBKEY_ENV);
}

function warnOnce(key: string, text: string): void {
  if (warned.has(key)) return;
  warned.add(key);
  logger.warn(undefined, text);
}

function isFileMissing(error: unknown): boolean {
  return isRecord(error) && error.code === "ENOENT";
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
