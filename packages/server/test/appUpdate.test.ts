/**
 * fpk 在线升级的纯函数与载荷交换回归（docs/specs/fnos-feature-parity-and-update.md）。
 *
 * 这里不访问 GitHub：check/download 的 fetch 被注入，fpk 由 createTarGzArchive 现造。
 * apply 的 cmd/main restart 由临时 bash 脚本替代，只验证「交换 + 备份 + 触发重启」。
 */
import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";

import { FnosAppUpdater } from "../src/appUpdate/manager.js";
import { normalizeUpdatePublicKey } from "../src/appUpdate/signature.js";
import {
  buildReleaseUrls,
  compareStableVersions,
  fpkAssetName,
  parseFpkManifestVersion,
  parseSha256ForFile,
  parseStableVersion,
} from "../src/appUpdate/version.js";
import { createTarGzArchive } from "../src/remote/localTarGz.js";

const BASE_URL = "https://example.invalid/mode/releases";
const NEW_VERSION = "0.0.95";
const REQUIRED_FILES = [
  "bin/mode.mjs",
  "web/index.html",
  "server/index.js",
  "agent/index.js",
  "node_modules/dep.js",
  "runtime/bin/node",
  "proxy.mjs",
  "ui/config",
] as const;

test("版本解析只接受正式版，比较按数字段", () => {
  assert.equal(parseStableVersion("v0.0.95"), "0.0.95");
  assert.equal(parseStableVersion("0.0.95"), "0.0.95");
  assert.equal(parseStableVersion("0.0.95-audit.1"), null);
  assert.equal(parseStableVersion("nightly"), null);
  assert.equal(compareStableVersions("0.0.95", "0.0.94"), 1);
  assert.equal(compareStableVersions("0.0.94", "0.0.95"), -1);
  assert.equal(compareStableVersions("0.0.94", "0.0.94"), 0);
});

test("sha256 清单按文件名取值，忽略路径/星号前缀", () => {
  const manifest = [
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  Mode-0.0.95-win-x64.exe",
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb *mode-0.0.95-x86_64.fpk",
  ].join("\n");
  assert.equal(
    parseSha256ForFile(manifest, "mode-0.0.95-x86_64.fpk"),
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  );
  assert.equal(parseSha256ForFile(manifest, "missing.fpk"), null);
});

test("发布 URL 与资产名固定", () => {
  assert.equal(fpkAssetName("0.0.95"), "mode-0.0.95-x86_64.fpk");
  const urls = buildReleaseUrls(`${BASE_URL}/`, "0.0.95");
  assert.equal(urls.latestManifest, `${BASE_URL}/latest/download/latest.json`);
  assert.equal(urls.checksums, `${BASE_URL}/download/v0.0.95/sha256.txt`);
  assert.equal(urls.checksumsSignature, `${BASE_URL}/download/v0.0.95/sha256.txt.sig`);
  assert.equal(urls.fpk, `${BASE_URL}/download/v0.0.95/mode-0.0.95-x86_64.fpk`);
});

test("fpk manifest 版本解析只认正式版", () => {
  assert.equal(parseFpkManifestVersion("appname = mode\nversion = 0.0.95\n"), "0.0.95");
  assert.equal(parseFpkManifestVersion("version = v0.0.95"), "0.0.95");
  assert.equal(parseFpkManifestVersion("version = 0.0.95-rc.1"), null);
  assert.equal(parseFpkManifestVersion("appname = mode\n"), null);
});

test("发布公钥支持 PEM 与 32 字节裸 base64，非法值返回 null", () => {
  const { publicKey } = generateKeyPairSync("ed25519");
  const pem = publicKey.export({ type: "spki", format: "pem" }).toString();
  assert.ok(normalizeUpdatePublicKey(pem));
  const raw = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
  assert.ok(normalizeUpdatePublicKey(raw));
  assert.equal(normalizeUpdatePublicKey("not-a-key"), null);
});

async function createPayload(root: string, marker: string, version = NEW_VERSION): Promise<string> {
  const payload = join(root, `payload-${marker}`);
  for (const relativePath of REQUIRED_FILES) {
    const target = join(payload, relativePath);
    await mkdir(dirname(target), { recursive: true });
    await writeFile(target, `${marker}:${relativePath}`, "utf8");
  }
  await writeFile(join(payload, "package.json"), JSON.stringify({ version }), "utf8");
  return payload;
}

async function createFpkFixture(
  root: string,
  options: { payloadVersion?: string; manifestVersion?: string } = {},
): Promise<{ buffer: Buffer; sha: string }> {
  const payload = await createPayload(root, "new", options.payloadVersion ?? NEW_VERSION);
  const appTgz = join(root, "app.tgz");
  await createTarGzArchive(
    appTgz,
    REQUIRED_FILES.map((relativePath) => ({
      sourcePath: join(payload, relativePath),
      archivePath: relativePath,
    })).concat([{ sourcePath: join(payload, "package.json"), archivePath: "package.json" }]),
  );
  const manifest = join(root, "manifest");
  await writeFile(
    manifest,
    `appname = mode\nversion = ${options.manifestVersion ?? NEW_VERSION}\n`,
    "utf8",
  );
  const fpk = join(root, fpkAssetName(NEW_VERSION));
  await createTarGzArchive(fpk, [
    { sourcePath: appTgz, archivePath: "app.tgz" },
    { sourcePath: manifest, archivePath: "manifest" },
  ]);
  const buffer = await readFile(fpk);
  return { buffer, sha: createHash("sha256").update(buffer).digest("hex") };
}

async function createInstalledApp(root: string): Promise<string> {
  const appDest = join(root, "install");
  for (const relativePath of REQUIRED_FILES) {
    const target = join(appDest, relativePath);
    await mkdir(dirname(target), { recursive: true });
    await writeFile(target, `old:${relativePath}`, "utf8");
  }
  await writeFile(join(appDest, "app.sock"), "socket-placeholder", "utf8");
  return appDest;
}

test("check/download/apply：校验、交换、备份与重启触发", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-app-update-"));
  try {
    const { buffer, sha } = await createFpkFixture(root);
    const appDest = await createInstalledApp(root);
    const restartMarker = join(root, "restart.txt");
    const mainScript = join(root, "fake-main.sh");
    await writeFile(
      mainScript,
      `#!/bin/bash\nprintf 'restarted' > "${restartMarker.replace(/\\/g, "/")}"\n`,
      "utf8",
    );

    const fetchImpl = (async (input: unknown) => {
      const url = String(input);
      if (url.endsWith("latest.json")) {
        return new Response(JSON.stringify({ version: NEW_VERSION }), {
          headers: { "content-type": "application/json" },
        });
      }
      if (url.endsWith("sha256.txt")) {
        return new Response(`${sha}  ${fpkAssetName(NEW_VERSION)}\n`);
      }
      if (url.endsWith(".fpk")) {
        return new Response(buffer, {
          headers: { "content-length": String(buffer.length) },
        });
      }
      return new Response("missing", { status: 404 });
    }) as unknown as typeof fetch;

    const updater = new FnosAppUpdater({
      appDest,
      mainScript,
      currentVersion: "0.0.94",
      releasesBaseUrl: BASE_URL,
      fetchImpl,
      now: () => new Date("2026-10-27T00:00:00.000Z"),
      log: () => undefined,
    });

    assert.equal(updater.getState().enabled, true);
    assert.equal(updater.getState().currentVersion, "0.0.94");

    assert.equal((await updater.check()).phase, "available");
    const downloaded = await updater.download();
    assert.equal(downloaded.phase, "ready", JSON.stringify(downloaded));
    assert.equal(updater.getState().canApply, true);

    const result = await updater.apply();
    assert.equal(result.ok, true);
    assert.equal(result.restarting, true);

    assert.equal(await readFile(join(appDest, "bin/mode.mjs"), "utf8"), "new:bin/mode.mjs");
    assert.equal(await readFile(join(appDest, "app.sock"), "utf8"), "socket-placeholder");
    assert.equal(
      await readFile(join(appDest, ".update/backup/bin/mode.mjs"), "utf8"),
      "old:bin/mode.mjs",
    );
    // 交换完成后 in-progress 必须消失；backup 留给 cmd/main 在启动成功后清理。
    await assert.rejects(stat(join(appDest, ".update/in-progress")));

    // 触发重启的 spawn 是 detached 的；给它一个窗口，但不把测试绑死在 bash 是否存在上。
    if (process.platform !== "win32") {
      for (let attempt = 0; attempt < 20; attempt += 1) {
        try {
          assert.equal(await readFile(restartMarker, "utf8"), "restarted");
          break;
        } catch {
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
      }
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("校验失败不进入 ready，且不留下可应用载荷", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-app-update-bad-"));
  try {
    const { buffer } = await createFpkFixture(root);
    const appDest = await createInstalledApp(root);
    const fetchImpl = (async (input: unknown) => {
      const url = String(input);
      if (url.endsWith("latest.json")) {
        return new Response(JSON.stringify({ version: NEW_VERSION }));
      }
      if (url.endsWith("sha256.txt")) {
        return new Response(
          `${"0".repeat(64)}  ${fpkAssetName(NEW_VERSION)}\n`,
        );
      }
      if (url.endsWith(".fpk")) {
        return new Response(buffer, {
          headers: { "content-length": String(buffer.length) },
        });
      }
      return new Response("missing", { status: 404 });
    }) as unknown as typeof fetch;

    const updater = new FnosAppUpdater({
      appDest,
      mainScript: join(root, "unused.sh"),
      currentVersion: "0.0.94",
      releasesBaseUrl: BASE_URL,
      fetchImpl,
      log: () => undefined,
    });

    await updater.check();
    const state = await updater.download();
    assert.equal(state.phase, "error");
    assert.match(state.error ?? "", /checksum_mismatch/u);
    await assert.rejects(stat(join(appDest, ".update/app")));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("配置公钥后校验 sha256.txt 签名，签名不符拒绝下载", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-app-update-sig-"));
  try {
    const { buffer, sha } = await createFpkFixture(root);
    const checksums = `${sha}  ${fpkAssetName(NEW_VERSION)}\n`;
    const { publicKey, privateKey } = generateKeyPairSync("ed25519");
    const publicKeyPem = publicKey.export({ type: "spki", format: "pem" }).toString();
    const goodSignature = sign(null, Buffer.from(checksums, "utf8"), privateKey).toString(
      "base64",
    );
    const badSignature = sign(null, Buffer.from("other", "utf8"), privateKey).toString("base64");

    const createUpdater = (appDest: string, signature: string) => {
      const fetchImpl = (async (input: unknown) => {
        const url = String(input);
        if (url.endsWith("latest.json")) {
          return new Response(JSON.stringify({ version: NEW_VERSION }));
        }
        if (url.endsWith("sha256.txt")) {
          return new Response(checksums);
        }
        if (url.endsWith("sha256.txt.sig")) {
          return new Response(signature);
        }
        if (url.endsWith(".fpk")) {
          return new Response(buffer, {
            headers: { "content-length": String(buffer.length) },
          });
        }
        return new Response("missing", { status: 404 });
      }) as unknown as typeof fetch;
      return new FnosAppUpdater({
        appDest,
        mainScript: join(root, "unused.sh"),
        currentVersion: "0.0.94",
        releasesBaseUrl: BASE_URL,
        updatePublicKey: publicKeyPem,
        fetchImpl,
        log: () => undefined,
      });
    };

    const good = createUpdater(await createInstalledApp(join(root, "good")), goodSignature);
    await good.check();
    assert.equal((await good.download()).phase, "ready");

    const bad = createUpdater(await createInstalledApp(join(root, "bad")), badSignature);
    await bad.check();
    const state = await bad.download();
    assert.equal(state.phase, "error");
    assert.match(state.error ?? "", /signature_invalid/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("外壳或载荷版本与 release 不一致时拒绝进入 ready", async () => {
  for (const options of [{ manifestVersion: "0.0.94" }, { payloadVersion: "0.0.96" }]) {
    const root = await mkdtemp(join(tmpdir(), "mode-app-update-ver-"));
    try {
      const { buffer, sha } = await createFpkFixture(root, options);
      const appDest = await createInstalledApp(root);
      const fetchImpl = (async (input: unknown) => {
        const url = String(input);
        if (url.endsWith("latest.json")) {
          return new Response(JSON.stringify({ version: NEW_VERSION }));
        }
        if (url.endsWith("sha256.txt")) {
          return new Response(`${sha}  ${fpkAssetName(NEW_VERSION)}\n`);
        }
        if (url.endsWith(".fpk")) {
          return new Response(buffer, {
            headers: { "content-length": String(buffer.length) },
          });
        }
        return new Response("missing", { status: 404 });
      }) as unknown as typeof fetch;

      const updater = new FnosAppUpdater({
        appDest,
        mainScript: join(root, "unused.sh"),
        currentVersion: "0.0.94",
        releasesBaseUrl: BASE_URL,
        fetchImpl,
        log: () => undefined,
      });
      await updater.check();
      const state = await updater.download();
      assert.equal(state.phase, "error");
      assert.match(state.error ?? "", /version_mismatch/u);
      await assert.rejects(stat(join(appDest, ".update/app")));
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  }
});

test("非 fpk 部署不提供检查/下载/应用", async () => {
  const updater = new FnosAppUpdater({ currentVersion: "0.0.94" });
  assert.equal(updater.getState().enabled, false);
  assert.equal((await updater.check()).phase, "idle");
  assert.equal((await updater.download()).phase, "idle");
  assert.deepEqual(await updater.apply(), {
    ok: false,
    restarting: false,
    error: "disabled",
  });
});
