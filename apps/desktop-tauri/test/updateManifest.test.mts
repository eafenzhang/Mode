/**
 * `latest.json` 生成器的回归网（M5 Task 1）。
 *
 * 这里钉住三件发布流程必须成立的事：
 * 1. 清单形状是 tauri-plugin-updater 读的 `{version, notes, pub_date, platforms.windows-x86_64}`
 *    且 `signature` 是 `.sig` 文件原文（不做二次 base64）；
 * 2. URL 必须是 https（release 构建会拒绝 http endpoint）；
 * 3. URL 文件名必须与安装包文件名一致（发布后排障靠它定位）。
 */
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const script = join(repoRoot, "scripts/tauri-update-manifest.ts");

function run(args: string[]) {
  return spawnSync(process.execPath, ["--import", "tsx", script, ...args], {
    cwd: repoRoot,
    encoding: "utf8",
  });
}

function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "mode-update-manifest-"));
  const artifact = join(dir, "Mode-0.0.2-win-x64.exe");
  const signature = `${artifact}.sig`;
  const output = join(dir, "latest.json");
  writeFileSync(artifact, "installer-bytes");
  writeFileSync(signature, "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZQpzaWc=\n");
  return { dir, artifact, signature, output };
}

test("writes the windows-x86_64 manifest with the signature file verbatim", () => {
  const { artifact, signature, output } = fixture();
  const result = run([
    "--version",
    "0.0.2",
    "--artifact",
    artifact,
    "--signature",
    signature,
    "--url",
    "https://example.test/Mode-0.0.2-win-x64.exe",
    "--output",
    output,
    "--notes",
    "drill",
    "--pub-date",
    "2026-10-17T00:00:00Z",
  ]);
  assert.equal(result.status, 0, result.stderr);
  const manifest = JSON.parse(readFileSync(output, "utf8"));
  assert.deepEqual(Object.keys(manifest).sort(), ["notes", "platforms", "pub_date", "version"]);
  assert.equal(manifest.version, "0.0.2");
  assert.equal(manifest.pub_date, "2026-10-17T00:00:00Z");
  assert.equal(manifest.notes, "drill");
  assert.deepEqual(Object.keys(manifest.platforms), ["windows-x86_64"]);
  assert.equal(
    manifest.platforms["windows-x86_64"].signature,
    readFileSync(signature, "utf8").trim(),
  );
  assert.equal(
    manifest.platforms["windows-x86_64"].url,
    "https://example.test/Mode-0.0.2-win-x64.exe",
  );
});

test("rejects a non-https URL", () => {
  const { artifact, signature, output } = fixture();
  const result = run([
    "--version",
    "0.0.2",
    "--artifact",
    artifact,
    "--signature",
    signature,
    "--url",
    "http://example.test/Mode-0.0.2-win-x64.exe",
    "--output",
    output,
  ]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /必须是 https/u);
});

test("rejects a URL whose filename does not match the artifact", () => {
  const { artifact, signature, output } = fixture();
  const result = run([
    "--version",
    "0.0.2",
    "--artifact",
    artifact,
    "--signature",
    signature,
    "--url",
    "https://example.test/other-name.exe",
    "--output",
    output,
  ]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /文件名必须与 --artifact 一致/u);
});
