/**
 * IM SDK 运行时在线升级：清单验签、tar.gz 安全解包、原子安装/回退与加载优先级。
 * 路径与边界口径见 `docs/specs/im-sdk-runtime.md`。
 */
import assert from "node:assert/strict";
import { generateKeyPairSync, sign } from "node:crypto";
import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import { gzipSync } from "node:zlib";

import {
  ImSdkRuntimeError,
  compareImSdkRuntimeVersions,
  parseImSdkRuntimeManifest,
  sha256Hex,
  verifyImSdkRuntimeManifestSignature,
} from "../src/bots/imSdkManifest.js";
import {
  resolveImSdkEntry,
  readActiveImSdkRuntime,
  readImSdkRuntimePointer,
} from "../src/bots/imSdkRuntime.js";
import { installImSdkRuntime, refreshImSdkRuntime } from "../src/bots/imSdkRuntimeUpgrade.js";
import { extractImSdkRuntimeTarGz } from "../src/bots/imSdkTar.js";

const { privateKey, publicKey } = generateKeyPairSync("ed25519");
const PUBLIC_KEY_BASE64 = Buffer.from(
  String(publicKey.export({ format: "jwk" }).x),
  "base64url",
).toString("base64");

const ENTRYPOINTS = {
  "@larksuiteoapi/node-sdk": "node_modules/@larksuiteoapi/node-sdk/index.js",
  "@wecom/aibot-node-sdk": "node_modules/@wecom/aibot-node-sdk/index.js",
} as const;
const SDK_VERSIONS = {
  "@larksuiteoapi/node-sdk": "1.64.0",
  "@wecom/aibot-node-sdk": "1.0.7",
} as const;

interface FakeBundle {
  version: string;
  manifestBytes: Buffer;
  signature: Buffer;
  bundleBytes: Buffer;
  files: Map<string, Buffer>;
}

/**
 * 生成一份可安装的假运行时包。`mutateTarFiles` 只改 tar 内容，清单仍按原始文件生成，
 * 用于构造篡改/多文件/缺文件场景。
 */
function createFakeBundle(
  version: string,
  mutateTarFiles?: (files: Map<string, Buffer>) => void,
): FakeBundle {
  const files = new Map<string, Buffer>([
    [
      "node_modules/@larksuiteoapi/node-sdk/package.json",
      Buffer.from(JSON.stringify({ name: "@larksuiteoapi/node-sdk", main: "index.js" })),
    ],
    [
      "node_modules/@larksuiteoapi/node-sdk/index.js",
      Buffer.from(
        `exports.EventDispatcher = function () {};\nexports.version = ${JSON.stringify(version)};\n`,
      ),
    ],
    [
      "node_modules/@wecom/aibot-node-sdk/package.json",
      Buffer.from(JSON.stringify({ name: "@wecom/aibot-node-sdk", main: "index.js" })),
    ],
    [
      "node_modules/@wecom/aibot-node-sdk/index.js",
      Buffer.from(
        `exports.WSClient = function () {};\nexports.generateReqId = function () { return ${JSON.stringify(version)}; };\n`,
      ),
    ],
  ]);
  const tarFiles = new Map([...files].map(([path, bytes]) => [path, Buffer.from(bytes)]));
  mutateTarFiles?.(tarFiles);
  const bundleBytes = createTarGz(tarFiles);
  const manifest = {
    schemaVersion: 1,
    version,
    createdAt: new Date().toISOString(),
    hostApiVersion: 1,
    sdkVersions: SDK_VERSIONS,
    entrypoints: ENTRYPOINTS,
    bundle: {
      file: `bundle-${version}.tar.gz`,
      size: bundleBytes.length,
      sha256: sha256Hex(bundleBytes),
    },
    files: [...files].map(([path, bytes]) => ({
      path,
      size: bytes.length,
      sha256: sha256Hex(bytes),
    })),
  };
  const manifestBytes = Buffer.from(JSON.stringify(manifest, null, 2));
  return {
    version,
    manifestBytes,
    // 与发布产物一致：签名文件是 base64 文本（不是原始 64 字节）。
    signature: Buffer.from(sign(null, manifestBytes, privateKey).toString("base64")),
    bundleBytes,
    files,
  };
}

function createTarGz(files: Map<string, Buffer>): Buffer {
  const chunks: Buffer[] = [];
  for (const [path, bytes] of files) {
    const header = createTarHeader(path, bytes.length, "0");
    chunks.push(header, bytes, Buffer.alloc((512 - (bytes.length % 512)) % 512));
  }
  chunks.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(chunks));
}

function createTarHeader(path: string, size: number, type: "0" | "2" | "5"): Buffer {
  assert.ok(Buffer.byteLength(path) <= 100, "test paths must fit the ustar name field");
  const header = Buffer.alloc(512);
  header.write(path, 0, 100, "utf8");
  header.write("0000644\0", 100, "ascii");
  header.write("0000000\0", 108, "ascii");
  header.write("0000000\0", 116, "ascii");
  header.write(`${size.toString(8).padStart(11, "0")}\0`, 124, "ascii");
  header.write("00000000000\0", 136, "ascii");
  header.write("        ", 148, "ascii");
  header.write(type, 156, "ascii");
  header.write("ustar\0", 257, "ascii");
  header.write("00", 263, "ascii");
  let checksum = 0;
  for (const byte of header) checksum += byte;
  header.write(`${checksum.toString(8).padStart(6, "0")}\0 `, 148, "ascii");
  return header;
}

function withTempRoot(run: (root: string) => Promise<void> | void): Promise<void> {
  const root = mkdtempSync(join(tmpdir(), "mode-im-sdk-runtime-"));
  return Promise.resolve(run(root)).finally(() => {
    rmSync(root, { recursive: true, force: true });
  });
}

async function install(root: string, version: string): Promise<void> {
  const bundle = createFakeBundle(version);
  const result = await installImSdkRuntime({
    root,
    manifestBytes: bundle.manifestBytes,
    signature: bundle.signature,
    bundleBytes: bundle.bundleBytes,
    publicKeyBase64: PUBLIC_KEY_BASE64,
  });
  assert.equal(result.status, "installed");
}

test("清单校验与签名：合法签名通过，篡改一个字节即失败", () => {
  const bundle = createFakeBundle("2026.11.20.1");
  const manifest = parseImSdkRuntimeManifest(bundle.manifestBytes);
  assert.equal(manifest.version, "2026.11.20.1");
  verifyImSdkRuntimeManifestSignature(
    bundle.manifestBytes,
    bundle.signature.toString("utf8"),
    PUBLIC_KEY_BASE64,
  );

  const tampered = Buffer.from(bundle.manifestBytes);
  tampered[tampered.length - 2] = tampered[tampered.length - 2] === 0x20 ? 0x21 : 0x20;
  assert.throws(
    () => verifyImSdkRuntimeManifestSignature(tampered, bundle.signature, PUBLIC_KEY_BASE64),
    (error: unknown) => error instanceof ImSdkRuntimeError && error.code === "invalid-signature",
  );
});

test("清单拒绝不兼容版本、保留文件名与不安全路径", () => {
  const base = JSON.parse(createFakeBundle("2026.11.20.1").manifestBytes.toString("utf8"));
  const mutate = (patch: Record<string, unknown>) => {
    const raw = { ...base, ...patch };
    return Buffer.from(JSON.stringify(raw));
  };
  assert.throws(
    () => parseImSdkRuntimeManifest(mutate({ hostApiVersion: 2 })),
    (error: unknown) => error instanceof ImSdkRuntimeError && error.code === "incompatible",
  );
  assert.throws(
    () =>
      parseImSdkRuntimeManifest(
        mutate({
          files: [{ path: "manifest.json", size: 1, sha256: "0".repeat(64) }],
        }),
      ),
    /reserved/,
  );
  assert.throws(
    () =>
      parseImSdkRuntimeManifest(
        mutate({
          entrypoints: { "@wecom/aibot-node-sdk": "../escape/package.json" },
        }),
      ),
    /safe relative path/,
  );
  assert.deepEqual(compareImSdkRuntimeVersions("2026.11.20.10", "2026.11.20.9"), 1);
  assert.deepEqual(compareImSdkRuntimeVersions("1.0", "1.0.0"), 0);
});

test("安装后 resolveImSdkEntry 指向运行时的包入口并可加载", async () => {
  await withTempRoot(async (root) => {
    await install(root, "2026.11.20.1");
    assert.equal(readImSdkRuntimePointer({ root })?.version, "2026.11.20.1");
    assert.equal(readActiveImSdkRuntime({ root })?.version, "2026.11.20.1");

    const entry = resolveImSdkEntry("@wecom/aibot-node-sdk", { root });
    assert.ok(entry?.includes(join("versions", "2026.11.20.1", "node_modules")));
    const module_ = await import(pathToFileURL(entry as string).href);
    assert.equal(module_.generateReqId(), "2026.11.20.1");
  });
});

test("升级切换入口；降级被拒绝且指针不变", async () => {
  await withTempRoot(async (root) => {
    await install(root, "2026.11.20.1");
    const oldEntry = resolveImSdkEntry("@larksuiteoapi/node-sdk", { root });
    await install(root, "2026.11.20.2");
    const newEntry = resolveImSdkEntry("@larksuiteoapi/node-sdk", { root });
    assert.notEqual(oldEntry, newEntry);
    assert.ok(newEntry?.includes("2026.11.20.2"));

    const downgrade = createFakeBundle("2026.11.20.1");
    await assert.rejects(
      installImSdkRuntime({
        root,
        manifestBytes: downgrade.manifestBytes,
        signature: downgrade.signature,
        bundleBytes: downgrade.bundleBytes,
        publicKeyBase64: PUBLIC_KEY_BASE64,
      }),
      (error: unknown) => error instanceof ImSdkRuntimeError && error.code === "downgrade",
    );
    assert.equal(readImSdkRuntimePointer({ root })?.version, "2026.11.20.2");
  });
});

test("篡改包/清单外文件被拒绝，active 指针保持不变", async () => {
  await withTempRoot(async (root) => {
    await install(root, "2026.11.20.1");

    const tampered = createFakeBundle("2026.11.20.2", (files) => {
      files.set(
        "node_modules/@wecom/aibot-node-sdk/index.js",
        Buffer.from("exports.WSClient = function () {};\n"),
      );
    });
    await assert.rejects(
      installImSdkRuntime({
        root,
        manifestBytes: tampered.manifestBytes,
        signature: tampered.signature,
        bundleBytes: tampered.bundleBytes,
        publicKeyBase64: PUBLIC_KEY_BASE64,
      }),
      (error: unknown) => error instanceof ImSdkRuntimeError && error.code === "checksum-mismatch",
    );

    const extra = createFakeBundle("2026.11.20.3", (files) => {
      files.set("node_modules/evil/index.js", Buffer.from("x"));
    });
    await assert.rejects(
      installImSdkRuntime({
        root,
        manifestBytes: extra.manifestBytes,
        signature: extra.signature,
        bundleBytes: extra.bundleBytes,
        publicKeyBase64: PUBLIC_KEY_BASE64,
      }),
      (error: unknown) => error instanceof ImSdkRuntimeError && error.code === "checksum-mismatch",
    );

    assert.equal(readImSdkRuntimePointer({ root })?.version, "2026.11.20.1");
    assert.deepEqual(readdirSync(join(root, "versions")), ["2026.11.20.1"]);
  });
});

test("解包拒绝路径穿越、符号链接与绝对路径", () => {
  const traversal = createTarGz(
    new Map([
      ["ok/file.js", Buffer.from("x")],
      ["../escape.js", Buffer.from("x")],
    ]),
  );
  assert.throws(
    () => extractImSdkRuntimeTarGz(traversal),
    (error: unknown) => error instanceof ImSdkRuntimeError && error.code === "unsafe-archive",
  );

  const symlink = gzipSync(
    Buffer.concat([createTarHeader("node_modules/pkg/link", 0, "2"), Buffer.alloc(1024)]),
  );
  assert.throws(
    () => extractImSdkRuntimeTarGz(symlink),
    (error: unknown) => error instanceof ImSdkRuntimeError && error.code === "unsafe-archive",
  );

  assert.throws(() => extractImSdkRuntimeTarGz(gzipSync(Buffer.alloc(1024))), /no files/);
});

test("GC 只保留 active + 一个旧版本", async () => {
  await withTempRoot(async (root) => {
    await install(root, "2026.11.20.1");
    await install(root, "2026.11.20.2");
    await install(root, "2026.11.20.3");
    assert.deepEqual(readdirSync(join(root, "versions")).sort(), ["2026.11.20.2", "2026.11.20.3"]);
  });
});

test("删除指针或 kill switch 时回退随包 SDK", async () => {
  await withTempRoot(async (root) => {
    await install(root, "2026.11.20.1");
    assert.ok(resolveImSdkEntry("@wecom/aibot-node-sdk", { root }));
    rmSync(join(root, "active.json"), { force: true });
    assert.equal(resolveImSdkEntry("@wecom/aibot-node-sdk", { root }), undefined);
    assert.equal(readActiveImSdkRuntime({ root }), undefined);

    await install(root, "2026.11.20.2");
    assert.equal(
      resolveImSdkEntry("@wecom/aibot-node-sdk", {
        root,
        env: { MODE_IM_SDK_RUNTIME_DISABLE: "1" },
      }),
      undefined,
    );
  });
});

test("refresh：未配置时 disabled，成功激活，签名失败时不改指针", async () => {
  await withTempRoot(async (root) => {
    assert.deepEqual(await refreshImSdkRuntime({ root, env: {} }), {
      status: "disabled",
      reason: "no-manifest-url",
    });
    assert.deepEqual(
      await refreshImSdkRuntime({
        root,
        env: { MODE_IM_SDK_RUNTIME_MANIFEST_URL: "https://x/m.json" },
      }),
      { status: "disabled", reason: "no-public-key" },
    );

    const bundle = createFakeBundle("2026.11.20.1");
    const manifestUrl = "https://updates.example.test/im-sdk/manifest.json";
    const fetchImpl = createFetch(mapOf(bundle, manifestUrl));
    const first = await refreshImSdkRuntime({
      root,
      manifestUrl,
      publicKeyBase64: PUBLIC_KEY_BASE64,
      fetchImpl,
    });
    assert.equal(first.status, "activated");
    assert.equal(readImSdkRuntimePointer({ root })?.version, "2026.11.20.1");

    const second = await refreshImSdkRuntime({
      root,
      manifestUrl,
      publicKeyBase64: PUBLIC_KEY_BASE64,
      fetchImpl,
    });
    assert.deepEqual(second, { status: "up-to-date", version: "2026.11.20.1" });

    const broken = createFakeBundle("2026.11.20.2");
    // 保持 base64 可解码，但签名字节整体错位：应验签失败而不是解析失败。
    const badSignature = Buffer.from(broken.signature.toString("utf8"));
    badSignature[0] = badSignature[0] === 0x41 ? 0x42 : 0x41;
    const failingFetch = createFetch(
      mapOf({ ...broken, signature: badSignature }, manifestUrl, "bundle-2026.11.20.2.tar.gz"),
    );
    const failed = await refreshImSdkRuntime({
      root,
      manifestUrl,
      publicKeyBase64: PUBLIC_KEY_BASE64,
      fetchImpl: failingFetch,
    });
    assert.equal(failed.status, "failed");
    assert.equal(readImSdkRuntimePointer({ root })?.version, "2026.11.20.1");
  });
});

function mapOf(
  bundle: FakeBundle,
  manifestUrl: string,
  bundleFile = `bundle-${bundle.version}.tar.gz`,
): Map<string, Buffer> {
  const base = manifestUrl.slice(0, manifestUrl.lastIndexOf("/") + 1);
  return new Map([
    [manifestUrl, bundle.manifestBytes],
    [`${manifestUrl}.sig`, bundle.signature],
    [`${base}${bundleFile}`, bundle.bundleBytes],
  ]);
}

function createFetch(responses: Map<string, Buffer>): typeof fetch {
  return (async (input: string | URL | Request) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const body = responses.get(url);
    if (!body) return new Response("not found", { status: 404 });
    return new Response(new Uint8Array(body), {
      status: 200,
      headers: { "content-length": String(body.length) },
    });
  }) as typeof fetch;
}
