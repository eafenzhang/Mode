/**
 * IM SDK 运行时清单的 Ed25519 签名工具（LT-7 T2）。
 *
 * 客户端（`packages/services/src/bots/imSdkManifest.ts`）校验的是覆盖
 * `manifest.json` 原始字节的裸 Ed25519 签名，签名文件是 base64 文本。
 *
 * 用法：
 * ```bash
 * pnpm im-sdk-runtime:sign generate --out .secrets
 * pnpm im-sdk-runtime:sign sign --manifest dist/im-sdk-runtime/im-sdk-runtime-manifest.json \
 *   --key-env MODE_IM_SDK_RUNTIME_SIGNING_KEY
 * pnpm im-sdk-runtime:sign verify --manifest ... --sig ... --pubkey <base64|文件>
 * ```
 * 私钥只从文件或环境变量读取，不写日志；CI 用 secret 注入环境变量。
 */
import { createPrivateKey, createPublicKey, generateKeyPairSync, sign, verify } from "node:crypto";
import { chmod, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

export const DEFAULT_SIGNING_KEY_ENV = "MODE_IM_SDK_RUNTIME_SIGNING_KEY";
export const PRIVATE_KEY_FILE_NAME = "im-sdk-runtime-signing-key.pem";
export const PUBLIC_KEY_FILE_NAME = "im-sdk-runtime-public-key.txt";

export interface GeneratedSigningKey {
  privateKeyPem: string;
  publicKeyBase64: string;
}

export function generateSigningKey(): GeneratedSigningKey {
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const privateKeyPem = privateKey.export({ type: "pkcs8", format: "pem" }).toString();
  const jwk = publicKey.export({ format: "jwk" }) as { x?: string };
  if (!jwk.x) throw new Error("ed25519 public key has no JWK x coordinate");
  return {
    privateKeyPem,
    publicKeyBase64: Buffer.from(jwk.x, "base64url").toString("base64"),
  };
}

export function signManifestBytes(manifestBytes: Buffer, privateKeyPem: string): string {
  const privateKey = createPrivateKey(privateKeyPem);
  if (privateKey.asymmetricKeyType !== "ed25519") {
    throw new Error(
      `signing key must be Ed25519, got ${privateKey.asymmetricKeyType ?? "unknown"}`,
    );
  }
  return sign(null, manifestBytes, privateKey).toString("base64");
}

export function verifyManifestSignature(
  manifestBytes: Buffer,
  signatureBase64: string,
  publicKeyBase64: string,
): void {
  const raw = Buffer.from(publicKeyBase64.replace(/\s+/gu, ""), "base64");
  if (raw.length !== 32) throw new Error(`public key must be 32 bytes, got ${raw.length}`);
  const publicKey = createPublicKey({
    key: { kty: "OKP", crv: "Ed25519", x: raw.toString("base64url") },
    format: "jwk",
  });
  const signatureBytes = Buffer.from(signatureBase64.replace(/\s+/gu, ""), "base64");
  if (signatureBytes.length !== 64) {
    throw new Error(`signature must be 64 bytes, got ${signatureBytes.length}`);
  }
  if (!verify(null, manifestBytes, publicKey, signatureBytes)) {
    throw new Error("manifest signature does not match the public key");
  }
}

async function readKeyFromArgs(args: Map<string, string>): Promise<string> {
  const keyFile = args.get("key");
  if (keyFile) return readFile(resolve(keyFile), "utf8");
  const envName = args.get("key-env") ?? DEFAULT_SIGNING_KEY_ENV;
  const fromEnv = process.env[envName]?.trim();
  if (fromEnv) return fromEnv;
  throw new Error(`missing signing key: pass --key <pem> or set ${envName}`);
}

async function commandGenerate(args: Map<string, string>): Promise<void> {
  const outDir = resolve(args.get("out") ?? ".secrets");
  const { privateKeyPem, publicKeyBase64 } = generateSigningKey();
  const privateKeyPath = resolve(outDir, PRIVATE_KEY_FILE_NAME);
  const publicKeyPath = resolve(outDir, PUBLIC_KEY_FILE_NAME);
  await writeFile(privateKeyPath, privateKeyPem, { encoding: "utf8", flag: "wx", mode: 0o600 });
  await chmod(privateKeyPath, 0o600).catch(() => undefined);
  await writeFile(publicKeyPath, `${publicKeyBase64}\n`, { encoding: "utf8", flag: "wx" });
  process.stdout.write(
    `${JSON.stringify({ privateKeyPath, publicKeyPath, publicKeyBase64 }, null, 2)}\n`,
  );
}

async function commandSign(args: Map<string, string>): Promise<void> {
  const manifestPath = args.get("manifest");
  if (!manifestPath) throw new Error("missing --manifest");
  const manifestBytes = await readFile(resolve(manifestPath));
  const privateKeyPem = await readKeyFromArgs(args);
  const signature = signManifestBytes(manifestBytes, privateKeyPem);
  const signaturePath = resolve(args.get("out") ?? `${resolve(manifestPath)}.sig`);
  await writeFile(signaturePath, `${signature}\n`);
  process.stdout.write(`${JSON.stringify({ signaturePath, bytes: manifestBytes.length })}\n`);
}

/** 从私钥推导客户端配置用的 raw Ed25519 公钥（base64，32 字节）。 */
async function commandPublicKey(args: Map<string, string>): Promise<void> {
  const privateKey = createPrivateKey(await readKeyFromArgs(args));
  if (privateKey.asymmetricKeyType !== "ed25519") {
    throw new Error(
      `signing key must be Ed25519, got ${privateKey.asymmetricKeyType ?? "unknown"}`,
    );
  }
  const jwk = createPublicKey(privateKey).export({ format: "jwk" }) as { x?: string };
  if (!jwk.x) throw new Error("ed25519 public key has no JWK x coordinate");
  process.stdout.write(`${Buffer.from(jwk.x, "base64url").toString("base64")}\n`);
}

async function commandVerify(args: Map<string, string>): Promise<void> {
  const manifestPath = args.get("manifest");
  const signaturePath = args.get("sig") ?? (manifestPath ? `${manifestPath}.sig` : undefined);
  const publicKeyArg = args.get("pubkey");
  if (!manifestPath || !signaturePath || !publicKeyArg) {
    throw new Error("verify needs --manifest, --sig (optional default) and --pubkey");
  }
  const manifestBytes = await readFile(resolve(manifestPath));
  const signature = await readFile(resolve(signaturePath), "utf8");
  const looksLikePath =
    publicKeyArg.endsWith(".txt") || publicKeyArg.includes("/") || publicKeyArg.includes("\\");
  const publicKeyBase64 = looksLikePath
    ? (await readFile(resolve(publicKeyArg), "utf8")).trim()
    : publicKeyArg;
  verifyManifestSignature(manifestBytes, signature, publicKeyBase64);
  process.stdout.write(`OK: ${manifestPath} (${manifestBytes.length} bytes)\n`);
}

function parseArgs(argv: string[]): { command: string; args: Map<string, string> } {
  const [command, ...rest] = argv;
  if (!command) throw new Error("usage: sign <generate|sign|verify> [...args]");
  const args = new Map<string, string>();
  for (let index = 0; index < rest.length; index += 1) {
    const key = rest[index];
    if (!key?.startsWith("--")) continue;
    const value = rest[index + 1];
    if (!value || value.startsWith("--")) throw new Error(`${key} needs a value`);
    args.set(key.slice(2), value);
    index += 1;
  }
  return { command, args };
}

async function main(): Promise<void> {
  const { command, args } = parseArgs(process.argv.slice(2));
  if (command === "generate") return commandGenerate(args);
  if (command === "sign") return commandSign(args);
  if (command === "public-key") return commandPublicKey(args);
  if (command === "verify") return commandVerify(args);
  throw new Error(`unknown command: ${command}`);
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : "";
if (import.meta.url === invokedPath) {
  main().catch((error: unknown) => {
    process.stderr.write(`[im-sdk-runtime:sign] ${String(error)}\n`);
    process.exitCode = 1;
  });
}
