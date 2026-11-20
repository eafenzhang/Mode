/**
 * IM SDK 运行时清单：解析、校验与 Ed25519 验签。
 *
 * 清单是发布侧对「这一包 SDK 里到底有什么」的完整声明；Node 侧只信任通过验签且
 * 文件集与清单逐项吻合的包。格式与安全边界见 `docs/specs/im-sdk-runtime.md` §3/§6。
 */
import { createHash, createPublicKey, verify, type KeyObject } from "node:crypto";

export const IM_SDK_RUNTIME_SCHEMA_VERSION = 1;
export const IM_SDK_RUNTIME_HOST_API_VERSION = 1;
/** 清单里禁止出现的保留文件名：它由安装器用验签过的原始字节写入。 */
export const IM_SDK_RUNTIME_MANIFEST_FILE_NAME = "manifest.json";

export type ImSdkRuntimeErrorCode =
  | "invalid-manifest"
  | "invalid-signature"
  | "incompatible"
  | "downgrade"
  | "checksum-mismatch"
  | "unsafe-archive"
  | "download-failed"
  | "io-error";

export class ImSdkRuntimeError extends Error {
  constructor(
    readonly code: ImSdkRuntimeErrorCode,
    message: string,
  ) {
    super(message);
    this.name = "ImSdkRuntimeError";
  }
}

export interface ImSdkRuntimeFileEntry {
  path: string;
  size: number;
  sha256: string;
}

export interface ImSdkRuntimeBundleRef {
  file: string;
  size: number;
  sha256: string;
}

export interface ImSdkRuntimeManifest {
  schemaVersion: number;
  version: string;
  createdAt: string;
  hostApiVersion: number;
  sdkVersions: Record<string, string>;
  entrypoints: Record<string, string>;
  bundle: ImSdkRuntimeBundleRef;
  files: ImSdkRuntimeFileEntry[];
}

const SHA256_HEX = /^[0-9a-f]{64}$/;
const VERSION = /^[0-9]+(\.[0-9]+)*$/;

export function invalidManifest(message: string): ImSdkRuntimeError {
  return new ImSdkRuntimeError("invalid-manifest", message);
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isValidImSdkRuntimeVersion(value: string): boolean {
  return VERSION.test(value);
}

export function sha256Hex(bytes: Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}

/**
 * 版本比较：点分十进制，逐段数值比较，缺段按 0 处理。
 * 返回负数/0/正数（与 `Array#sort` 的比较器同向）。
 */
export function compareImSdkRuntimeVersions(left: string, right: string): number {
  const leftParts = parseVersionParts(left);
  const rightParts = parseVersionParts(right);
  const length = Math.max(leftParts.length, rightParts.length);
  for (let index = 0; index < length; index += 1) {
    const a = leftParts[index] ?? 0;
    const b = rightParts[index] ?? 0;
    if (a !== b) return a < b ? -1 : 1;
  }
  return 0;
}

function parseVersionParts(value: string): number[] {
  if (!isValidImSdkRuntimeVersion(value)) {
    throw invalidManifest(`version "${value}" must match ${VERSION.source}`);
  }
  return value.split(".").map((part) => {
    const parsed = Number(part);
    if (!Number.isSafeInteger(parsed)) {
      throw invalidManifest(`version segment "${part}" is too large`);
    }
    return parsed;
  });
}

/**
 * 包内相对路径的安全判定：拒绝绝对路径、盘符、反斜杠、`.`/`..` 段与 NUL。
 * 解包器与清单校验共用这一条规则，避免两套口径。
 */
export function isSafeBundleRelativePath(value: string): boolean {
  if (!value || value.length > 4096 || value.includes("\\") || value.includes("\0")) {
    return false;
  }
  if (value.startsWith("/") || /^[A-Za-z]:/.test(value)) return false;
  const segments = value.split("/");
  return segments.every(
    (segment) => segment.length > 0 && segment.length <= 255 && segment !== "." && segment !== "..",
  );
}

/** 包文件名只能是单段文件名：下载 URL 由清单文件名与清单 URL 拼接得到。 */
export function isSafeBundleFileName(value: string): boolean {
  return isSafeBundleRelativePath(value) && !value.includes("/");
}

export function parseImSdkRuntimeManifest(bytes: Buffer): ImSdkRuntimeManifest {
  let raw: unknown;
  try {
    raw = JSON.parse(bytes.toString("utf8"));
  } catch {
    throw invalidManifest("manifest is not valid JSON");
  }
  if (!isRecord(raw)) throw invalidManifest("manifest root must be an object");
  if (raw.schemaVersion !== IM_SDK_RUNTIME_SCHEMA_VERSION) {
    throw new ImSdkRuntimeError(
      "incompatible",
      `schemaVersion ${String(raw.schemaVersion)} is not supported`,
    );
  }
  if (raw.hostApiVersion !== IM_SDK_RUNTIME_HOST_API_VERSION) {
    throw new ImSdkRuntimeError(
      "incompatible",
      `hostApiVersion ${String(raw.hostApiVersion)} is not supported`,
    );
  }
  if (typeof raw.version !== "string" || !isValidImSdkRuntimeVersion(raw.version)) {
    throw invalidManifest("version is missing or malformed");
  }
  if (
    typeof raw.createdAt !== "string" ||
    raw.createdAt.trim() === "" ||
    Number.isNaN(Date.parse(raw.createdAt))
  ) {
    throw invalidManifest("createdAt is missing or not an ISO timestamp");
  }
  const sdkVersions = parseStringRecord(raw.sdkVersions, "sdkVersions");
  const entrypoints = parseStringRecord(raw.entrypoints, "entrypoints");
  if (Object.keys(entrypoints).length === 0) {
    throw invalidManifest("entrypoints must not be empty");
  }
  const files = parseFileEntries(raw.files);
  const filePaths = new Set(files.map((file) => file.path));
  for (const [packageName, entrypoint] of Object.entries(entrypoints)) {
    if (!isSafeBundleRelativePath(entrypoint)) {
      throw invalidManifest(`entrypoint for ${packageName} is not a safe relative path`);
    }
    if (!filePaths.has(entrypoint)) {
      throw invalidManifest(`entrypoint for ${packageName} is not listed in files`);
    }
    if (!(packageName in sdkVersions)) {
      throw invalidManifest(`entrypoint for ${packageName} has no sdkVersions entry`);
    }
  }
  if (!isRecord(raw.bundle)) throw invalidManifest("bundle is missing");
  if (typeof raw.bundle.file !== "string" || !isSafeBundleFileName(raw.bundle.file)) {
    throw invalidManifest("bundle.file must be a single safe file name");
  }
  if (!isValidSize(raw.bundle.size)) throw invalidManifest("bundle.size is invalid");
  if (typeof raw.bundle.sha256 !== "string" || !SHA256_HEX.test(raw.bundle.sha256)) {
    throw invalidManifest("bundle.sha256 is invalid");
  }
  return {
    schemaVersion: IM_SDK_RUNTIME_SCHEMA_VERSION,
    version: raw.version,
    createdAt: raw.createdAt,
    hostApiVersion: IM_SDK_RUNTIME_HOST_API_VERSION,
    sdkVersions,
    entrypoints,
    bundle: {
      file: raw.bundle.file,
      size: raw.bundle.size,
      sha256: raw.bundle.sha256,
    },
    files,
  };
}

function parseStringRecord(value: unknown, field: string): Record<string, string> {
  if (!isRecord(value)) throw invalidManifest(`${field} must be an object`);
  const result: Record<string, string> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (key.trim() === "" || typeof entry !== "string" || entry.trim() === "") {
      throw invalidManifest(`${field}.${key} must be a non-empty string`);
    }
    result[key] = entry;
  }
  return result;
}

function parseFileEntries(value: unknown): ImSdkRuntimeFileEntry[] {
  if (!Array.isArray(value) || value.length === 0) {
    throw invalidManifest("files must be a non-empty array");
  }
  const seen = new Set<string>();
  return value.map((entry, index) => {
    if (!isRecord(entry)) throw invalidManifest(`files[${index}] must be an object`);
    const path = entry.path;
    if (typeof path !== "string" || !isSafeBundleRelativePath(path)) {
      throw invalidManifest(`files[${index}].path is not a safe relative path`);
    }
    if (path === IM_SDK_RUNTIME_MANIFEST_FILE_NAME) {
      throw invalidManifest(`${IM_SDK_RUNTIME_MANIFEST_FILE_NAME} is reserved`);
    }
    if (seen.has(path)) throw invalidManifest(`duplicate file path: ${path}`);
    seen.add(path);
    if (!isValidSize(entry.size)) throw invalidManifest(`files[${index}].size is invalid`);
    if (typeof entry.sha256 !== "string" || !SHA256_HEX.test(entry.sha256)) {
      throw invalidManifest(`files[${index}].sha256 is invalid`);
    }
    return { path, size: entry.size, sha256: entry.sha256 };
  });
}

function isValidSize(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

/** 解码 base64 raw Ed25519 公钥（32 字节）。 */
export function decodeImSdkRuntimePublicKey(base64Value: string): KeyObject {
  const normalized = base64Value.replace(/\s+/g, "");
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(normalized)) {
    throw new ImSdkRuntimeError("invalid-signature", "public key is not valid base64");
  }
  const bytes = Buffer.from(normalized, "base64");
  if (bytes.length !== 32) {
    throw new ImSdkRuntimeError(
      "invalid-signature",
      `public key must be 32 raw bytes, got ${bytes.length}`,
    );
  }
  try {
    return createPublicKey({
      key: { kty: "OKP", crv: "Ed25519", x: bytes.toString("base64url") },
      format: "jwk",
    });
  } catch (error) {
    throw new ImSdkRuntimeError("invalid-signature", `cannot decode public key: ${message(error)}`);
  }
}

/**
 * 校验 detached 签名：签名覆盖 `manifestBytes` 原始字节。
 * `signature` 是 **base64 文本**（string 或包含文本的 Buffer，允许换行），
 * 与发布产物里的 `.sig` 文件字节一致；不接受裸 64 字节签名。
 */
export function verifyImSdkRuntimeManifestSignature(
  manifestBytes: Buffer,
  signature: string | Buffer,
  publicKeyBase64: string,
): void {
  const publicKey = decodeImSdkRuntimePublicKey(publicKeyBase64);
  const signatureText = (
    typeof signature === "string" ? signature : signature.toString("utf8")
  ).replace(/\s+/g, "");
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(signatureText)) {
    throw new ImSdkRuntimeError("invalid-signature", "signature is not valid base64");
  }
  const signatureBytes = Buffer.from(signatureText, "base64");
  if (signatureBytes.length !== 64) {
    throw new ImSdkRuntimeError(
      "invalid-signature",
      `signature must be 64 bytes, got ${signatureBytes.length}`,
    );
  }
  let valid = false;
  try {
    valid = verify(null, manifestBytes, publicKey, signatureBytes);
  } catch (error) {
    throw new ImSdkRuntimeError("invalid-signature", `signature check failed: ${message(error)}`);
  }
  if (!valid) {
    throw new ImSdkRuntimeError(
      "invalid-signature",
      "manifest signature does not match the configured public key",
    );
  }
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
