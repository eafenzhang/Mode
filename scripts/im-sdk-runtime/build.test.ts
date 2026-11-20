/**
 * IM SDK 运行时发布流水线（LT-7）：打包器 → 清单 → 签名 → 本地 HTTP → 客户端安装/激活。
 *
 * 这个用例跨发布脚本与客户端运行时，验证两边的清单/哈希/签名口径一致；
 * 不依赖公网、真实 SDK 或真实密钥。
 */
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { buildImSdkRuntimeBundle } from "./build.js";
import { generateSigningKey, signManifestBytes, verifyManifestSignature } from "./sign.js";
import {
  parseImSdkRuntimeManifest,
  verifyImSdkRuntimeManifestSignature,
} from "../../packages/services/src/bots/imSdkManifest.js";
import {
  readImSdkRuntimePointer,
  resolveImSdkEntry,
} from "../../packages/services/src/bots/imSdkRuntime.js";
import { refreshImSdkRuntime } from "../../packages/services/src/bots/imSdkRuntimeUpgrade.js";

const LARK = "@larksuiteoapi/node-sdk";
const WECOM = "@wecom/aibot-node-sdk";
const SDK_PACKAGES = [
  { name: LARK, version: "1.64.0" },
  { name: WECOM, version: "1.0.7" },
];

async function withTemp(run: (root: string) => Promise<void>): Promise<void> {
  const root = await mkdtemp(join(tmpdir(), "im-sdk-pipeline-"));
  try {
    await run(root);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

async function createFixtureNodeModules(root: string, larkVersion = "1.64.0"): Promise<string> {
  const files: Record<string, string> = {
    [`${LARK}/package.json`]: JSON.stringify({
      name: LARK,
      version: larkVersion,
      main: "./lib/index.js",
    }),
    [`${LARK}/lib/index.js`]: `exports.sdkVersion = ${JSON.stringify(larkVersion)};\n`,
    [`${WECOM}/package.json`]: JSON.stringify({
      name: WECOM,
      version: "1.0.7",
      main: "dist/index.cjs.js",
    }),
    [`${WECOM}/dist/index.cjs.js`]:
      'exports.WSClient = function () {};\nexports.generateReqId = function () { return "req"; };\n',
    ["ws/package.json"]: JSON.stringify({ name: "ws", version: "8.18.0", main: "index.js" }),
    ["ws/index.js"]: "module.exports = { name: 'ws' };\n",
  };
  const nodeModules = join(root, "node_modules");
  for (const [relativePath, content] of Object.entries(files)) {
    const absolutePath = join(nodeModules, relativePath);
    await mkdir(dirname(absolutePath), { recursive: true });
    await writeFile(absolutePath, content);
  }
  return nodeModules;
}

async function buildAndSign(version: string, nodeModulesDir: string, outDir: string) {
  const result = await buildImSdkRuntimeBundle({
    version,
    outDir,
    nodeModulesDir,
    packages: SDK_PACKAGES,
    createdAt: "2026-11-21T00:00:00.000Z",
  });
  const manifestBytes = await readFile(result.manifestPath);
  const key = generateSigningKey();
  const signature = signManifestBytes(manifestBytes, key.privateKeyPem);
  const bundleBytes = await readFile(result.bundlePath);
  return { result, manifestBytes, signature, key, bundleBytes };
}

async function serveBundle(files: Map<string, Buffer>): Promise<{ server: Server; url: string }> {
  const server = createServer((request, response) => {
    const path = new URL(request.url ?? "/", "http://localhost").pathname;
    const body = files.get(path);
    if (!body) {
      response.statusCode = 404;
      response.end("not found");
      return;
    }
    response.setHeader("content-length", String(body.length));
    response.end(body);
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  return { server, url: `http://127.0.0.1:${port}/im-sdk-runtime-manifest.json` };
}

test("打包器产出可被客户端解析的清单，签名可双向校验", async () => {
  await withTemp(async (root) => {
    const nodeModules = await createFixtureNodeModules(root);
    const { result, manifestBytes, signature, key } = await buildAndSign(
      "2026.11.21.1",
      nodeModules,
      join(root, "out"),
    );
    const manifest = parseImSdkRuntimeManifest(manifestBytes);
    assert.equal(manifest.version, "2026.11.21.1");
    assert.equal(manifest.sdkVersions[LARK], "1.64.0");
    assert.equal(manifest.entrypoints[LARK], `node_modules/${LARK}/lib/index.js`);
    assert.equal(manifest.entrypoints[WECOM], `node_modules/${WECOM}/dist/index.cjs.js`);
    assert.ok(manifest.files.some((file) => file.path === manifest.entrypoints[LARK]));
    assert.equal(manifest.bundle.sha256, result.manifest.bundle.sha256);

    verifyImSdkRuntimeManifestSignature(manifestBytes, signature, key.publicKeyBase64);
    verifyManifestSignature(manifestBytes, signature, key.publicKeyBase64);

    const tampered = Buffer.from(manifestBytes);
    tampered[tampered.length - 2] = tampered[tampered.length - 2] === 0x20 ? 0x21 : 0x20;
    assert.throws(() =>
      verifyImSdkRuntimeManifestSignature(tampered, signature, key.publicKeyBase64),
    );
    assert.throws(() => verifyManifestSignature(tampered, signature, key.publicKeyBase64));
  });
});

test("本地清单经 refresh 安装并激活，运行时入口可加载", async () => {
  await withTemp(async (root) => {
    const nodeModules = await createFixtureNodeModules(root);
    const { result, manifestBytes, signature, key, bundleBytes } = await buildAndSign(
      "2026.11.21.1",
      nodeModules,
      join(root, "out"),
    );
    const manifest = parseImSdkRuntimeManifest(manifestBytes);
    const { server, url } = await serveBundle(
      new Map([
        ["/im-sdk-runtime-manifest.json", manifestBytes],
        ["/im-sdk-runtime-manifest.json.sig", Buffer.from(signature)],
        [`/${manifest.bundle.file}`, bundleBytes],
      ]),
    );
    try {
      const dataRoot = join(root, "data");
      const refresh = await refreshImSdkRuntime({
        root: dataRoot,
        manifestUrl: url,
        publicKeyBase64: key.publicKeyBase64,
        env: {},
      });
      assert.equal(refresh.status, "activated");
      assert.equal(readImSdkRuntimePointer({ root: dataRoot })?.version, "2026.11.21.1");
      const entry = resolveImSdkEntry(LARK, { root: dataRoot, env: {} });
      if (!entry) throw new Error("runtime entry should resolve");
      const module_ = await import(pathToFileURL(entry).href);
      assert.equal(module_.sdkVersion, "1.64.0");
      const wecomEntry = resolveImSdkEntry(WECOM, { root: dataRoot, env: {} });
      if (!wecomEntry) throw new Error("wecom runtime entry should resolve");
      const wecom = await import(pathToFileURL(wecomEntry).href);
      assert.equal(typeof wecom.WSClient, "function");

      const second = await refreshImSdkRuntime({
        root: dataRoot,
        manifestUrl: url,
        publicKeyBase64: key.publicKeyBase64,
        env: {},
      });
      assert.deepEqual(second, { status: "up-to-date", version: "2026.11.21.1" });
      assert.ok(result.fileCount >= 6);
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
});

test("bundle 被篡改时 refresh 失败且不切换 active 版本", async () => {
  await withTemp(async (root) => {
    const nodeModules = await createFixtureNodeModules(root);
    const first = await buildAndSign("2026.11.21.1", nodeModules, join(root, "out1"));
    const dataRoot = join(root, "data");
    const firstManifest = parseImSdkRuntimeManifest(first.manifestBytes);
    const firstServer = await serveBundle(
      new Map([
        ["/im-sdk-runtime-manifest.json", first.manifestBytes],
        ["/im-sdk-runtime-manifest.json.sig", Buffer.from(first.signature)],
        [`/${firstManifest.bundle.file}`, first.bundleBytes],
      ]),
    );
    try {
      const activated = await refreshImSdkRuntime({
        root: dataRoot,
        manifestUrl: firstServer.url,
        publicKeyBase64: first.key.publicKeyBase64,
        env: {},
      });
      assert.equal(activated.status, "activated");
    } finally {
      await new Promise<void>((resolve) => firstServer.server.close(() => resolve()));
    }

    const second = await buildAndSign("2026.11.21.2", nodeModules, join(root, "out2"));
    const secondManifest = parseImSdkRuntimeManifest(second.manifestBytes);
    const corrupted = Buffer.from(second.bundleBytes);
    corrupted[0] = corrupted[0] === 0 ? 1 : 0;
    const secondServer = await serveBundle(
      new Map([
        ["/im-sdk-runtime-manifest.json", second.manifestBytes],
        ["/im-sdk-runtime-manifest.json.sig", Buffer.from(second.signature)],
        [`/${secondManifest.bundle.file}`, corrupted],
      ]),
    );
    try {
      const failed = await refreshImSdkRuntime({
        root: dataRoot,
        manifestUrl: secondServer.url,
        publicKeyBase64: second.key.publicKeyBase64,
        env: {},
      });
      assert.equal(failed.status, "failed");
      assert.equal(readImSdkRuntimePointer({ root: dataRoot })?.version, "2026.11.21.1");
    } finally {
      await new Promise<void>((resolve) => secondServer.server.close(() => resolve()));
    }
  });
});
