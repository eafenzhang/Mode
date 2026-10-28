import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";

import { buildGithubAssetLayout, toGithubArtifactName } from "../publish-remote-assets.mjs";

const VERSION = "0.0.94";
const COMPONENT_ID = "server-bundle";
const COMPONENT_VERSION = "v0.0.94+abc123def456";
const ARTIFACT_RELATIVE_PATH = `components/linux-x64/${COMPONENT_ID}/${COMPONENT_VERSION}.tar.gz`;
const COMPONENT_BODY = "server-bundle-fixture";

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function createFixture(t, { sha256Value = sha256(COMPONENT_BODY) } = {}) {
  const root = mkdtempSync(join(tmpdir(), "mode-remote-assets-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const sourceDir = join(root, "mock-cdn");
  const releaseDir = join(sourceDir, "releases", VERSION);
  mkdirSync(releaseDir, { recursive: true });
  const artifactPath = join(sourceDir, ...ARTIFACT_RELATIVE_PATH.split("/"));
  mkdirSync(dirname(artifactPath), { recursive: true });
  writeFileSync(artifactPath, COMPONENT_BODY);
  writeFileSync(
    join(releaseDir, "manifest-linux-x64.json"),
    `${JSON.stringify(
      {
        schemaVersion: 1,
        appVersion: VERSION,
        platformArch: "linux-x64",
        components: [
          {
            id: COMPONENT_ID,
            version: COMPONENT_VERSION,
            sha256: sha256Value,
            sourceSha256: sha256Value,
            artifactPath: ARTIFACT_RELATIVE_PATH,
            mount: "server",
          },
        ],
      },
      null,
      2,
    )}\n`,
  );
  return { root, sourceDir, outDir: join(root, "out") };
}

test("把 mock-cdn 目录布局转换为 GitHub Release 扁平布局", (t) => {
  const fixture = createFixture(t);
  const result = buildGithubAssetLayout({
    sourceDir: fixture.sourceDir,
    outDir: fixture.outDir,
    expectedVersion: VERSION,
    platforms: ["linux-x64"],
  });

  const artifactName = toGithubArtifactName("linux-x64", COMPONENT_ID, COMPONENT_VERSION);
  assert.equal(artifactName, "zz-linux-x64__server-bundle__v0.0.94-abc123def456.tar.gz");
  assert.deepEqual(result.files.sort(), [
    "zz-linux-x64__server-bundle__v0.0.94-abc123def456.tar.gz",
    "zz-manifest-linux-x64.json",
  ]);
  const outputManifest = JSON.parse(
    readFileSync(join(fixture.outDir, "zz-manifest-linux-x64.json"), "utf8"),
  );
  assert.equal(outputManifest.appVersion, VERSION);
  assert.equal(outputManifest.components[0].artifactPath, artifactName);
  assert.ok(existsSync(join(fixture.outDir, artifactName)));
});

test("组件 sha256 不匹配时拒绝发布", (t) => {
  const fixture = createFixture(t, { sha256Value: "0".repeat(64) });
  assert.throws(
    () =>
      buildGithubAssetLayout({
        sourceDir: fixture.sourceDir,
        outDir: fixture.outDir,
        expectedVersion: VERSION,
        platforms: ["linux-x64"],
      }),
    /sha256 mismatch/,
  );
});

test("manifest 版本与发布版本不一致时拒绝发布", (t) => {
  const fixture = createFixture(t);
  assert.throws(
    () =>
      buildGithubAssetLayout({
        sourceDir: fixture.sourceDir,
        outDir: fixture.outDir,
        expectedVersion: "0.0.95",
        platforms: ["linux-x64"],
      }),
    /release directory not found/,
  );
});
