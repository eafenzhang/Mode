/**
 * CUA Helper 打包管线回归（Plan C Task 1）：
 * 1) electron-builder extraResources 仅 win32 目标携带 cua-helper 条目
 *    （from=bundled-tools/<key>/cua-helper → to=tools/cua-helper，filter 全量拷贝）；
 * 2) 非 win32（darwin/linux）目标的 extraResources 绝不引用 cua-helper 目录，
 *    否则 mac/linux 打包会因 source 不存在直接失败；
 * 3) stage 脚本幂等：二次运行后目标三件字节 sha 不变、manifest sha 不变；
 * 4) stage 源缺失时明确报错并指向 build:cua-helper（dev 自建路径）；
 * 5) 根 package.json 把 build:cua-helper 与 stage 串联（一键 build+stage）；
 * 6) 真实 dist / staged 产物（存在时）三件齐且 sha 与 manifest 一致。
 *
 * 运行：node --test packages/desktop/tests/cua-helper-packaging.test.mts
 * 扩展名说明：语言政策（docs/specs/language-policy.md）新代码仅允许 TS 等，
 * 禁入 .py/.cs/.js/.mjs/.cjs —— 故本测试用 .mts（Node 24 type-stripping 原生可跑）。
 */
import { after, test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dirname, "../../..");
const desktopRoot = join(repoRoot, "packages", "desktop");
const distCuaHelperDir = join(repoRoot, "packages", "mode-cua", "dist-cua-helper");
const stagedCuaHelperDir = join(desktopRoot, "bundled-tools", "win32-x64", "cua-helper");
const configUrl = new URL("../electron-builder.config.js", import.meta.url);
const stageScriptUrl = new URL("../../../scripts/stage-cua-helper.mts", import.meta.url);

interface ExtraResourceEntry {
  from?: string;
  to?: string;
  filter?: string[];
}

interface DesktopBuilderConfig {
  extraResources?: ExtraResourceEntry[];
}

const originalTargetOs = process.env.MODE_TARGET_OS;
const originalTargetArch = process.env.MODE_TARGET_ARCH;

after(() => {
  // 测试进程内改写过的目标平台变量不能泄漏给同进程的后续断言。
  if (originalTargetOs === undefined) delete process.env.MODE_TARGET_OS;
  else process.env.MODE_TARGET_OS = originalTargetOs;
  if (originalTargetArch === undefined) delete process.env.MODE_TARGET_ARCH;
  else process.env.MODE_TARGET_ARCH = originalTargetArch;
});

async function loadDesktopBuilderConfig(targetOs: string): Promise<DesktopBuilderConfig> {
  process.env.MODE_TARGET_OS = targetOs;
  process.env.MODE_TARGET_ARCH = "x64";
  // electron-builder.config 在模块求值时读一次目标平台；query 破缓存让
  // win32/darwin/linux 三种目标在同一测试进程内各自求值一份导出对象。
  const module = (await import(`${configUrl.href}?target=${targetOs}`)) as {
    default: DesktopBuilderConfig;
  };
  return module.default;
}

function isCuaHelperEntry(entry: ExtraResourceEntry): boolean {
  return `${entry.from ?? ""} ${entry.to ?? ""}`.includes("cua-helper");
}

function sha256Bytes(bytes: string | Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function sha256File(filePath: string): string {
  return sha256Bytes(readFileSync(filePath));
}

function snapshotDir(dirPath: string): Record<string, string> {
  const snapshot: Record<string, string> = {};
  for (const name of readdirSync(dirPath).sort()) {
    snapshot[name] = sha256File(join(dirPath, name));
  }
  return snapshot;
}

function createDistFixture(): { root: string; sourceDir: string } {
  const root = mkdtempSync(join(tmpdir(), "mode-cua-stage-fixture-"));
  const sourceDir = join(root, "dist-cua-helper");
  mkdirSync(sourceDir, { recursive: true });
  const entry = "console.log('cua fixture entry');\n";
  const addon = Buffer.from([0x4d, 0x5a, 0x90, 0x00, 0x03, 0x00]);
  const manifest = {
    schemaVersion: 1,
    packageName: "@mode/cua",
    packageVersion: "0.0.0-fixture",
    platform: "win32",
    arch: "x64",
    electronVersion: "41.0.3",
    entry: "entry.cjs",
    addon: "cua_ax.node",
    sha256: {
      entry: sha256Bytes(entry),
      addon: sha256Bytes(addon),
    },
  };
  writeFileSync(join(sourceDir, "entry.cjs"), entry);
  writeFileSync(join(sourceDir, "cua_ax.node"), addon);
  writeFileSync(join(sourceDir, "runtime-manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  return { root, sourceDir };
}

async function loadStageCuaHelper(): Promise<
  (options?: { sourceDir?: string; targetDir?: string }) => {
    targetDir: string;
    files: string[];
    manifestSha256: string;
  }
> {
  const module = (await import(stageScriptUrl.href)) as {
    stageCuaHelper: (options?: { sourceDir?: string; targetDir?: string }) => {
      targetDir: string;
      files: string[];
      manifestSha256: string;
    };
  };
  return module.stageCuaHelper;
}

test("extraResources：win32 目标携带 cua-helper 条目且 filter 正确", async () => {
  const config = await loadDesktopBuilderConfig("win32");
  const entries = (config.extraResources ?? []).filter(isCuaHelperEntry);
  assert.equal(entries.length, 1, "win32 目标必须恰好携带 1 个 cua-helper 条目");
  assert.equal(entries[0]?.from, "bundled-tools/win32-x64/cua-helper");
  assert.equal(entries[0]?.to, "tools/cua-helper");
  assert.deepEqual(entries[0]?.filter, ["**/*"], "filter 必须原样打包三件套");
});

test("extraResources：非 win32 目标不得引用 cua-helper 目录", async () => {
  for (const targetOs of ["darwin", "linux"]) {
    const config = await loadDesktopBuilderConfig(targetOs);
    const refs = (config.extraResources ?? []).filter(isCuaHelperEntry);
    assert.deepEqual(
      refs,
      [],
      `${targetOs} 构建的 extraResources 不得引用 bundled-tools cua-helper（否则打包因 source 缺失失败）`,
    );
  }
});

test("stage 幂等：二次运行三件齐、逐件与 manifest sha 不变", async () => {
  const stageCuaHelper = await loadStageCuaHelper();
  const { root, sourceDir } = createDistFixture();
  try {
    const targetDir = join(root, "bundled-tools", "win32-x64", "cua-helper");
    const first = stageCuaHelper({ sourceDir, targetDir });
    const firstSnapshot = snapshotDir(targetDir);
    const second = stageCuaHelper({ sourceDir, targetDir });
    const secondSnapshot = snapshotDir(targetDir);

    assert.deepEqual(Object.keys(secondSnapshot), [
      "cua_ax.node",
      "entry.cjs",
      "runtime-manifest.json",
    ]);
    assert.equal(first.manifestSha256, sha256File(join(targetDir, "runtime-manifest.json")));
    assert.equal(
      second.manifestSha256,
      first.manifestSha256,
      "二次 stage 的 manifest sha 必须不变",
    );
    assert.deepEqual(secondSnapshot, firstSnapshot, "二次 stage 后三件字节 sha 必须一致");
    assert.deepEqual(second.files.sort(), ["cua_ax.node", "entry.cjs", "runtime-manifest.json"]);
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});

test("stage 源缺失 → 明确报错并指向 build:cua-helper", async () => {
  const stageCuaHelper = await loadStageCuaHelper();
  const root = mkdtempSync(join(tmpdir(), "mode-cua-stage-missing-"));
  try {
    assert.throws(
      () =>
        stageCuaHelper({
          sourceDir: join(root, "not-built"),
          targetDir: join(root, "target"),
        }),
      /build:cua-helper/u,
      "源缺失必须给出「先运行 pnpm build:cua-helper」的自建指引",
    );
    assert.ok(!existsSync(join(root, "target")), "源缺失时不得留下半成品 stage 目录");
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});

test("根 package.json：build:cua-helper 自动串联 stage 并提供独立 stage 入口", () => {
  const pkg = JSON.parse(readFileSync(join(repoRoot, "package.json"), "utf8")) as {
    scripts: Record<string, string>;
  };
  assert.equal(
    typeof pkg.scripts["stage:cua-helper"],
    "string",
    "必须提供一键 stage 入口 pnpm stage:cua-helper",
  );
  assert.match(pkg.scripts["stage:cua-helper"] ?? "", /stage-cua-helper\.mts/u);
  assert.match(
    pkg.scripts["build:cua-helper"] ?? "",
    /stage-cua-helper\.mts/u,
    "build:cua-helper 完成后必须自动 stage（一键 build+stage）",
  );
});

test("ensure-local：win32 缺 helper 时打印自建/dev 指引（非致命兜底）", () => {
  const source = readFileSync(
    join(desktopRoot, "scripts", "ensure-local-runtime-assets.mjs"),
    "utf8",
  );
  assert.match(source, /MODE_CUA_DEV_ROOT/u, "dev 指引必须包含 MODE_CUA_DEV_ROOT 设置方法");
  assert.match(source, /build:cua-helper/u, "缺 dist 时必须指向 pnpm build:cua-helper 自建命令");
  assert.match(source, /stage:cua-helper/u, "缺 staged 产物时必须指向 pnpm stage:cua-helper");
  assert.match(
    source,
    /target\.os === "win32"/u,
    "CUA 兜底必须限定 win32（非 Windows dev 不提示）",
  );
});

test("真实 dist（若已构建）三件齐且 sha 与 manifest 一致", (t) => {
  const manifestPath = join(distCuaHelperDir, "runtime-manifest.json");
  if (!existsSync(manifestPath)) {
    t.skip("dist-cua-helper 未构建（CI verify 不跑 cargo）：跳过真实产物断言");
    return;
  }
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as {
    entry: string;
    addon: string;
    sha256: { entry: string; addon: string };
  };
  for (const name of ["entry.cjs", "cua_ax.node", "runtime-manifest.json"]) {
    assert.ok(existsSync(join(distCuaHelperDir, name)), `dist 三件齐：缺 ${name}`);
  }
  assert.equal(manifest.entry, "entry.cjs");
  assert.equal(manifest.addon, "cua_ax.node");
  assert.equal(sha256File(join(distCuaHelperDir, "entry.cjs")), manifest.sha256.entry);
  assert.equal(sha256File(join(distCuaHelperDir, "cua_ax.node")), manifest.sha256.addon);
});

test("真实 staged 目录（若已 stage）与 dist 逐件一致", (t) => {
  const stagedManifestPath = join(stagedCuaHelperDir, "runtime-manifest.json");
  if (
    !existsSync(stagedManifestPath) ||
    !existsSync(join(distCuaHelperDir, "runtime-manifest.json"))
  ) {
    t.skip("staged 或 dist 缺失：跳过真实 stage 一致性断言");
    return;
  }
  assert.deepEqual(
    snapshotDir(stagedCuaHelperDir),
    snapshotDir(distCuaHelperDir),
    "bundled-tools stage 必须与 dist-cua-helper 逐件字节一致",
  );
});
