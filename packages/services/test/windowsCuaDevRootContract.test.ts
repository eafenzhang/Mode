import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  resolveWindowsCuaRuntime,
  WindowsCuaDevRuntimeResolutionError,
} from "../src/cua-permission-broker/windowsCuaDevRuntime.js";

// 借真实消费方锁 Task 9 契约：产物缺失时跳过（先跑 pnpm build:cua-helper 生成
// packages/mode-cua/dist-cua-helper），避免未构建贡献者的 pnpm --filter @mode/services test 误报失败。
// brief 草稿写的是 "../../packages/mode-cua"，但从本文件（packages/services/test/）上溯两级
// 已到 packages/，再拼 packages/ 会得到 packages/packages/mode-cua（永久缺失→误跳过），故修正为 "../../mode-cua"。
const modeCuaRoot = fileURLToPath(new URL("../../mode-cua", import.meta.url));
const distRoot = join(modeCuaRoot, "dist-cua-helper");
const requiredArtifacts = ["entry.cjs", "cua_ax.node", "runtime-manifest.json"].map((file) =>
  join(distRoot, file),
);
const missingBuild = requiredArtifacts.some((file) => !existsSync(file));
const skip: string | false =
  missingBuild && "dist-cua-helper 缺失：先运行 pnpm build:cua-helper 生成 CUA helper 产物";

/** 与 scripts/build-cua-helper.mjs 的 REQUIRED_NAPI_VERSION 同源；改 crate features 时两处一起改。 */
const REQUIRED_NAPI_VERSION = 4;

function sha256Bytes(bytes: string | Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

/**
 * 造一个只满足 manifest 契约的最小产品布局（不依赖本机是否构建过 helper），
 * 用来覆盖 N-API 闸门的放行/拒绝分支。
 */
async function createPackagedFixture(manifestOverrides: Record<string, unknown> = {}): Promise<{
  resourcesPath: string;
  manifest: Record<string, unknown>;
  cleanup: () => Promise<void>;
}> {
  const resourcesPath = await realpath(await mkdtemp(join(tmpdir(), "mode-cua-napi-fixture-")));
  const helperRoot = join(resourcesPath, "tools", "cua-helper");
  await mkdir(helperRoot, { recursive: true });
  const entry = "console.log('cua fixture entry');\n";
  const addon = Buffer.from([0x4d, 0x5a, 0x90, 0x00, 0x03, 0x00]);
  const manifest = {
    schemaVersion: 2,
    packageName: "@mode/cua",
    packageVersion: "0.0.0-fixture",
    platform: "win32",
    arch: "x64",
    napiVersion: REQUIRED_NAPI_VERSION,
    entry: "entry.cjs",
    addon: "cua_ax.node",
    sha256: { entry: sha256Bytes(entry), addon: sha256Bytes(addon) },
    ...manifestOverrides,
  };
  await writeFile(join(helperRoot, "entry.cjs"), entry);
  await writeFile(join(helperRoot, "cua_ax.node"), addon);
  await writeFile(
    join(helperRoot, "runtime-manifest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );
  return {
    resourcesPath,
    manifest,
    cleanup: () => rm(resourcesPath, { recursive: true, force: true }),
  };
}

test("MODE_CUA_DEV_ROOT 按 modeCuaRuntime 契约解析构建产物", { skip }, async () => {
  const runtime = await resolveWindowsCuaRuntime({
    platform: "win32",
    arch: "x64",
    env: { ...process.env, MODE_CUA_DEV_ROOT: modeCuaRoot },
  });
  assert.equal(runtime.root, resolve(modeCuaRoot));
  assert.match(runtime.entryPath, /entry\.cjs$/u);
  assert.match(runtime.addonPath, /cua_ax\.node$/u);
  assert.equal(runtime.commandEnv.ELECTRON_RUN_AS_NODE, "1");
});

test("runtime-manifest 通过打包模式校验（sha256 对齐真实文件字节）", { skip }, async () => {
  const manifest = JSON.parse(
    await readFile(join(distRoot, "runtime-manifest.json"), "utf8"),
  ) as Record<string, unknown>;
  const modeCuaPkg = JSON.parse(await readFile(join(modeCuaRoot, "package.json"), "utf8")) as {
    name?: string;
    version?: string;
  };
  assert.equal(manifest.schemaVersion, 2);
  assert.equal(manifest.packageName, "@mode/cua");
  assert.equal(manifest.packageVersion, modeCuaPkg.version);
  assert.equal(manifest.platform, "win32");
  assert.equal(manifest.arch, "x64");
  assert.equal(manifest.napiVersion, REQUIRED_NAPI_VERSION);
  assert.equal(manifest.entry, "entry.cjs");
  assert.equal(manifest.addon, "cua_ax.node");

  // 打包模式读取 resources/tools/cua-helper：把构建产物按产品布局复制进临时 resources，
  // 让真实消费方对复制后的文件字节重算 sha256，验证与构建脚本的哈希口径完全一致。
  // 本机 TMP 是 8.3 短路径（ADMINI~1）：消费者要求 root 与 realpath 物理一致（fail-closed
  // 防 junction 逃逸），短路径会被 realpath 展开成长路径而误判，故先归一到规范长路径。
  const resourcesPath = await realpath(await mkdtemp(join(tmpdir(), "mode-cua-helper-resources-")));
  try {
    const helperRoot = join(resourcesPath, "tools", "cua-helper");
    await mkdir(helperRoot, { recursive: true });
    for (const file of ["entry.cjs", "cua_ax.node", "runtime-manifest.json"]) {
      await copyFile(join(distRoot, file), join(helperRoot, file));
    }
    const runtime = await resolveWindowsCuaRuntime({
      platform: "win32",
      arch: "x64",
      // 显式空 env 钉死打包分支：不传时消费方回落读 process.env，外壳若导出
      // MODE_CUA_DEV_ROOT 会把本用例误路由进 dev 分支（fail closed 报错），
      // 对本功能的目标受众恰是环境耦合的偶发失败。
      env: {},
      resourcesPath,
    });
    assert.equal(runtime.root, helperRoot);
    assert.match(runtime.entryPath, /entry\.cjs$/u);
    assert.match(runtime.addonPath, /cua_ax\.node$/u);
    assert.equal(runtime.commandEnv.ELECTRON_RUN_AS_NODE, "1");
  } finally {
    await rm(resourcesPath, { recursive: true, force: true });
  }
});

test("runtime-manifest v2：纯 Node 宿主按 N-API 版本放行（无 electron 版本）", async () => {
  const fixture = await createPackagedFixture();
  try {
    const runtime = await resolveWindowsCuaRuntime({
      platform: "win32",
      arch: "x64",
      env: {},
      resourcesPath: fixture.resourcesPath,
      napiVersion: REQUIRED_NAPI_VERSION,
    });
    assert.equal(runtime.root, join(fixture.resourcesPath, "tools", "cua-helper"));
  } finally {
    await fixture.cleanup();
  }
});

test("runtime-manifest v2：宿主 N-API 低于下限时 fail-closed", async () => {
  const fixture = await createPackagedFixture();
  try {
    await assert.rejects(
      resolveWindowsCuaRuntime({
        platform: "win32",
        arch: "x64",
        env: {},
        resourcesPath: fixture.resourcesPath,
        napiVersion: REQUIRED_NAPI_VERSION - 1,
      }),
      (error: unknown) => {
        assert.ok(error instanceof WindowsCuaDevRuntimeResolutionError);
        assert.equal(error.reason, "incompatible-runtime-manifest");
        return true;
      },
    );
  } finally {
    await fixture.cleanup();
  }
});

test("runtime-manifest v2：取不到宿主 N-API 版本时 fail-closed", async () => {
  const fixture = await createPackagedFixture();
  try {
    await assert.rejects(
      resolveWindowsCuaRuntime({
        platform: "win32",
        arch: "x64",
        env: {},
        resourcesPath: fixture.resourcesPath,
        napiVersion: Number.NaN,
      }),
      (error: unknown) => {
        assert.ok(error instanceof WindowsCuaDevRuntimeResolutionError);
        assert.equal(error.reason, "incompatible-runtime-manifest");
        return true;
      },
    );
  } finally {
    await fixture.cleanup();
  }
});

test("runtime-manifest v1（electronVersion）不再接受", async () => {
  const fixture = await createPackagedFixture({
    schemaVersion: 1,
    napiVersion: undefined,
    electronVersion: "41.0.3",
  });
  try {
    await assert.rejects(
      resolveWindowsCuaRuntime({
        platform: "win32",
        arch: "x64",
        env: {},
        resourcesPath: fixture.resourcesPath,
        napiVersion: REQUIRED_NAPI_VERSION,
      }),
      (error: unknown) => {
        assert.ok(error instanceof WindowsCuaDevRuntimeResolutionError);
        assert.equal(error.reason, "incompatible-runtime-manifest");
        return true;
      },
    );
  } finally {
    await fixture.cleanup();
  }
});
