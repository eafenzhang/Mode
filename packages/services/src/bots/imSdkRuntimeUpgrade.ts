/**
 * IM SDK 运行时安装与在线刷新（写入侧）。
 *
 * 流程见 `docs/specs/im-sdk-runtime.md` §5：验签清单 → 校验包哈希 → 安全解包 →
 * 逐文件比对 → 原子落版本目录 → 原子写 active 指针 → GC。任一步失败都不改动指针。
 */
import { randomBytes } from "node:crypto";
import { mkdir, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

import { createServiceLogger } from "../logger/serviceLogger.js";
import {
  IM_SDK_RUNTIME_MANIFEST_FILE_NAME,
  ImSdkRuntimeError,
  compareImSdkRuntimeVersions,
  isValidImSdkRuntimeVersion,
  parseImSdkRuntimeManifest,
  sha256Hex,
  verifyImSdkRuntimeManifestSignature,
  type ImSdkRuntimeManifest,
} from "./imSdkManifest.js";
import {
  invalidateImSdkRuntimeCache,
  isImSdkRuntimeDisabled,
  readActiveImSdkRuntime,
  readImSdkRuntimeManifestUrl,
  readImSdkRuntimePointer,
  readImSdkRuntimePublicKey,
  resolveImSdkRuntimeLayout,
  type ImSdkRuntimePointer,
  type ImSdkRuntimeScope,
} from "./imSdkRuntime.js";
import { IM_SDK_TAR_MAX_BYTES, extractImSdkRuntimeTarGz } from "./imSdkTar.js";

const logger = createServiceLogger("bots:im-sdk-upgrade");
const MAX_MANIFEST_BYTES = 4 * 1024 * 1024;
const MAX_SIGNATURE_BYTES = 64 * 1024;
const MAX_BUNDLE_BYTES = IM_SDK_TAR_MAX_BYTES;
const MANIFEST_TIMEOUT_MS = 30_000;
const BUNDLE_TIMEOUT_MS = 5 * 60_000;

export interface InstallImSdkRuntimeOptions {
  root?: string;
  manifestBytes: Buffer;
  signature: string | Buffer;
  bundleBytes: Buffer;
  publicKeyBase64: string;
  now?: () => Date;
}

export interface InstallImSdkRuntimeResult {
  status: "installed" | "up-to-date";
  version: string;
  previousVersion?: string;
}

export async function installImSdkRuntime(
  options: InstallImSdkRuntimeOptions,
): Promise<InstallImSdkRuntimeResult> {
  verifyImSdkRuntimeManifestSignature(
    options.manifestBytes,
    options.signature,
    options.publicKeyBase64,
  );
  const manifest = parseImSdkRuntimeManifest(options.manifestBytes);
  const scope: ImSdkRuntimeScope = { root: options.root };
  const layout = resolveImSdkRuntimeLayout(scope);
  const pointer = readImSdkRuntimePointer(scope);
  const currentVersion = pointer?.version;

  if (currentVersion) {
    const order = compareImSdkRuntimeVersions(manifest.version, currentVersion);
    if (order < 0) {
      throw new ImSdkRuntimeError(
        "downgrade",
        `refusing to downgrade IM SDK runtime ${currentVersion} -> ${manifest.version}`,
      );
    }
    if (order === 0 && readActiveImSdkRuntime(scope)?.version === manifest.version) {
      return { status: "up-to-date", version: manifest.version };
    }
    // 同版本但目录不完整：继续走安装流程覆盖修复。
  }

  if (options.bundleBytes.length !== manifest.bundle.size) {
    throw checksumError(
      `bundle size mismatch: manifest ${manifest.bundle.size}, download ${options.bundleBytes.length}`,
    );
  }
  if (sha256Hex(options.bundleBytes) !== manifest.bundle.sha256) {
    throw checksumError("bundle sha256 does not match the manifest");
  }
  const files = extractImSdkRuntimeTarGz(options.bundleBytes);
  verifyExtractedFiles(files, manifest);

  await mkdir(layout.versionsDir, { recursive: true });
  const target = join(layout.versionsDir, manifest.version);
  const staging = join(layout.versionsDir, `.staging-${randomBytes(6).toString("hex")}`);
  try {
    await writeStagingDirectory(staging, files, options.manifestBytes);
    await verifyEntrypointsExist(staging, manifest);
    // 同版本修复路径：目标目录可能半损坏；rename 前先清掉。
    await rm(target, { recursive: true, force: true });
    await rename(staging, target);
  } catch (error) {
    await rm(staging, { recursive: true, force: true }).catch(() => undefined);
    throw error;
  }

  const activatedAt = (options.now?.() ?? new Date()).toISOString();
  await writeActivePointer(layout.activeFile, {
    schemaVersion: 1,
    version: manifest.version,
    activatedAt,
    bundleSha256: manifest.bundle.sha256,
  });
  invalidateImSdkRuntimeCache();
  await collectGarbage(layout.versionsDir, manifest.version, currentVersion);
  return {
    status: "installed",
    version: manifest.version,
    ...(currentVersion && currentVersion !== manifest.version
      ? { previousVersion: currentVersion }
      : {}),
  };
}

export type ImSdkRuntimeRefreshResult =
  | { status: "disabled"; reason: "disabled-by-env" | "no-manifest-url" | "no-public-key" }
  | { status: "up-to-date"; version: string }
  | { status: "activated"; version: string; previousVersion?: string }
  | { status: "failed"; error: string };

export interface RefreshImSdkRuntimeOptions extends ImSdkRuntimeScope {
  manifestUrl?: string;
  publicKeyBase64?: string;
  fetchImpl?: typeof fetch;
  now?: () => Date;
}

/**
 * 检查并安装新版本。网络/校验失败只返回 `failed`，不清除已安装版本、不抛出。
 * `disabled` 表示没配置（正常形态：随包 SDK 生效）。
 */
export async function refreshImSdkRuntime(
  options: RefreshImSdkRuntimeOptions = {},
): Promise<ImSdkRuntimeRefreshResult> {
  const env = options.env ?? process.env;
  if (isImSdkRuntimeDisabled(env)) return { status: "disabled", reason: "disabled-by-env" };
  const manifestUrl = options.manifestUrl?.trim() || readImSdkRuntimeManifestUrl(env);
  if (!manifestUrl) return { status: "disabled", reason: "no-manifest-url" };
  const publicKeyBase64 = options.publicKeyBase64?.trim() || readImSdkRuntimePublicKey(env);
  if (!publicKeyBase64) return { status: "disabled", reason: "no-public-key" };

  try {
    assertAllowedRuntimeUrl(manifestUrl);
    const fetchImpl = options.fetchImpl ?? fetch;
    const manifestBytes = await fetchWithLimit(
      fetchImpl,
      manifestUrl,
      MAX_MANIFEST_BYTES,
      MANIFEST_TIMEOUT_MS,
    );
    const signature = await fetchWithLimit(
      fetchImpl,
      `${manifestUrl}.sig`,
      MAX_SIGNATURE_BYTES,
      MANIFEST_TIMEOUT_MS,
    );
    verifyImSdkRuntimeManifestSignature(manifestBytes, signature, publicKeyBase64);
    const manifest = parseImSdkRuntimeManifest(manifestBytes);

    const scope: ImSdkRuntimeScope = { root: options.root, env };
    const pointer = readImSdkRuntimePointer(scope);
    if (pointer) {
      const order = compareImSdkRuntimeVersions(manifest.version, pointer.version);
      if (order < 0) return { status: "up-to-date", version: pointer.version };
      if (order === 0 && readActiveImSdkRuntime(scope)) {
        return { status: "up-to-date", version: pointer.version };
      }
    }

    const bundleUrl = new URL(manifest.bundle.file, manifestUrl).toString();
    assertAllowedRuntimeUrl(bundleUrl);
    const bundleBytes = await fetchWithLimit(
      fetchImpl,
      bundleUrl,
      MAX_BUNDLE_BYTES,
      BUNDLE_TIMEOUT_MS,
    );
    const result = await installImSdkRuntime({
      ...(options.root ? { root: options.root } : {}),
      manifestBytes,
      signature,
      bundleBytes,
      publicKeyBase64,
      ...(options.now ? { now: options.now } : {}),
    });
    if (result.status === "up-to-date") {
      return { status: "up-to-date", version: result.version };
    }
    logger.info(
      undefined,
      `IM SDK runtime activated: ${result.version}` +
        (result.previousVersion ? ` (previous ${result.previousVersion})` : ""),
    );
    return {
      status: "activated",
      version: result.version,
      ...(result.previousVersion ? { previousVersion: result.previousVersion } : {}),
    };
  } catch (error) {
    const text = errorText(error);
    logger.warn(undefined, `IM SDK runtime refresh failed: ${text}`);
    return { status: "failed", error: text };
  }
}

/** Host 启动时的可选入口：未配置清单/公钥时什么都不做，不阻断启动。 */
export function maybeRefreshImSdkRuntimeInBackground(
  options: RefreshImSdkRuntimeOptions = {},
): void {
  const env = options.env ?? process.env;
  if (isImSdkRuntimeDisabled(env)) return;
  const manifestUrl = options.manifestUrl?.trim() || readImSdkRuntimeManifestUrl(env);
  const publicKeyBase64 = options.publicKeyBase64?.trim() || readImSdkRuntimePublicKey(env);
  if (!manifestUrl || !publicKeyBase64) return;
  void refreshImSdkRuntime({ ...options, env, manifestUrl, publicKeyBase64 }).catch((error) => {
    logger.warn(undefined, `IM SDK runtime background refresh crashed: ${errorText(error)}`);
  });
}

function verifyExtractedFiles(files: Map<string, Buffer>, manifest: ImSdkRuntimeManifest): void {
  if (files.size !== manifest.files.length) {
    throw checksumError(
      `file count mismatch: manifest ${manifest.files.length}, archive ${files.size}`,
    );
  }
  for (const entry of manifest.files) {
    const bytes = files.get(entry.path);
    if (!bytes) throw checksumError(`missing file from archive: ${entry.path}`);
    if (bytes.length !== entry.size) {
      throw checksumError(`size mismatch for ${entry.path}`);
    }
    if (sha256Hex(bytes) !== entry.sha256) {
      throw checksumError(`sha256 mismatch for ${entry.path}`);
    }
  }
}

async function writeStagingDirectory(
  staging: string,
  files: Map<string, Buffer>,
  manifestBytes: Buffer,
): Promise<void> {
  await mkdir(staging, { recursive: true });
  for (const [relativePath, bytes] of files) {
    const destination = join(staging, ...relativePath.split("/"));
    await mkdir(dirname(destination), { recursive: true });
    await writeFile(destination, bytes, { mode: 0o644 });
  }
  await writeFile(join(staging, IM_SDK_RUNTIME_MANIFEST_FILE_NAME), manifestBytes, { mode: 0o644 });
}

async function verifyEntrypointsExist(
  directory: string,
  manifest: ImSdkRuntimeManifest,
): Promise<void> {
  for (const [packageName, entrypoint] of Object.entries(manifest.entrypoints)) {
    const info = await stat(join(directory, ...entrypoint.split("/"))).catch(() => undefined);
    if (!info?.isFile()) {
      throw new ImSdkRuntimeError(
        "invalid-manifest",
        `entrypoint for ${packageName} is missing from the bundle`,
      );
    }
  }
}

async function writeActivePointer(activeFile: string, pointer: ImSdkRuntimePointer): Promise<void> {
  const temp = `${activeFile}.tmp-${randomBytes(4).toString("hex")}`;
  await writeFile(temp, `${JSON.stringify(pointer, null, 2)}\n`, { mode: 0o600 });
  try {
    await rename(temp, activeFile);
  } catch {
    // Windows 上 rename 覆盖已存在文件可能失败；指针是单写者，删除后重命名仍是原子的。
    await rm(activeFile, { force: true }).catch(() => undefined);
    await rename(temp, activeFile);
  }
}

/**
 * 保留 active + 上一个 active（回滚目标）；上一个 active 不可用时退回最高旧版本。
 * 清理 staging 残留。失败只告警，不影响已激活版本。
 */
async function collectGarbage(
  versionsDir: string,
  activeVersion: string,
  previousVersion?: string,
): Promise<void> {
  const entries = await readdir(versionsDir, { withFileTypes: true }).catch(() => []);
  const installed = entries
    .filter((entry) => entry.isDirectory() && isValidImSdkRuntimeVersion(entry.name))
    .map((entry) => entry.name)
    .sort(compareImSdkRuntimeVersions)
    .reverse();
  const keep = new Set([activeVersion]);
  const rollbackTarget =
    previousVersion && installed.includes(previousVersion)
      ? previousVersion
      : installed.find((version) => version !== activeVersion);
  if (rollbackTarget) keep.add(rollbackTarget);
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    const path = join(versionsDir, entry.name);
    const removable = entry.name.startsWith(".staging-") || !keep.has(entry.name);
    if (!removable) continue;
    await rm(path, { recursive: true, force: true }).catch((error) => {
      logger.warn(undefined, `cannot clean IM SDK runtime directory ${path}: ${errorText(error)}`);
    });
  }
}

function checksumError(text: string): ImSdkRuntimeError {
  return new ImSdkRuntimeError("checksum-mismatch", text);
}

function downloadError(text: string): ImSdkRuntimeError {
  return new ImSdkRuntimeError("download-failed", text);
}

/** 只允许 https；开发/测试可指向本机 http。凭据不允许放在 URL 里。 */
function assertAllowedRuntimeUrl(value: string): void {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw downloadError(`not a valid URL: ${value}`);
  }
  const localHost =
    url.hostname === "127.0.0.1" ||
    url.hostname === "localhost" ||
    url.hostname === "[::1]" ||
    url.hostname === "::1";
  if (url.protocol !== "https:" && !(url.protocol === "http:" && localHost)) {
    throw downloadError(`refusing insecure URL: ${value}`);
  }
  if (url.username || url.password) {
    throw downloadError("URL must not embed credentials");
  }
}

async function fetchWithLimit(
  fetchImpl: typeof fetch,
  url: string,
  maxBytes: number,
  timeoutMs: number,
): Promise<Buffer> {
  const response = await fetchImpl(url, {
    signal: AbortSignal.timeout(timeoutMs),
    redirect: "follow",
  });
  if (!response.ok) {
    throw downloadError(`GET ${url} failed with status ${response.status}`);
  }
  const declared = Number(response.headers.get("content-length") ?? "");
  if (Number.isFinite(declared) && declared > maxBytes) {
    throw downloadError(`response from ${url} exceeds ${maxBytes} bytes`);
  }
  if (!response.body) {
    const buffer = Buffer.from(await response.arrayBuffer());
    if (buffer.length > maxBytes) {
      throw downloadError(`response from ${url} exceeds ${maxBytes} bytes`);
    }
    return buffer;
  }
  const reader = response.body.getReader();
  const chunks: Buffer[] = [];
  let total = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!value) continue;
      total += value.byteLength;
      if (total > maxBytes) {
        await reader.cancel().catch(() => undefined);
        throw downloadError(`response from ${url} exceeds ${maxBytes} bytes`);
      }
      chunks.push(Buffer.from(value));
    }
  } finally {
    reader.releaseLock();
  }
  return Buffer.concat(chunks, total);
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
