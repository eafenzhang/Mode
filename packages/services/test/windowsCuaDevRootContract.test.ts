import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { resolveWindowsCuaRuntime } from "../src/cua-permission-broker/windowsCuaDevRuntime.js";

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

/** 与构建脚本同源推导桌面 electron 版本：env 覆盖优先，其次 packages/desktop/package.json（剥版本范围前缀）。 */
async function deriveDesktopElectronVersion(): Promise<string> {
  const override = process.env.MODE_CUA_ELECTRON_VERSION?.trim();
  if (override) return override;
  const desktopPkg = JSON.parse(
    await readFile(fileURLToPath(new URL("../../desktop/package.json", import.meta.url)), "utf8"),
  ) as { devDependencies?: Record<string, string>; dependencies?: Record<string, string> };
  const declared = desktopPkg.devDependencies?.electron ?? desktopPkg.dependencies?.electron;
  assert.ok(declared, "packages/desktop/package.json 必须声明 electron 版本");
  return declared.replace(/^[\^~>=<\s]+/u, "");
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
  const electronVersion = await deriveDesktopElectronVersion();
  const manifest = JSON.parse(
    await readFile(join(distRoot, "runtime-manifest.json"), "utf8"),
  ) as Record<string, unknown>;
  const modeCuaPkg = JSON.parse(await readFile(join(modeCuaRoot, "package.json"), "utf8")) as {
    name?: string;
    version?: string;
  };
  assert.equal(manifest.schemaVersion, 1);
  assert.equal(manifest.packageName, "@mode/cua");
  assert.equal(manifest.packageVersion, modeCuaPkg.version);
  assert.equal(manifest.platform, "win32");
  assert.equal(manifest.arch, "x64");
  assert.equal(manifest.electronVersion, electronVersion);
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
      resourcesPath,
      electronVersion,
    });
    assert.equal(runtime.root, helperRoot);
    assert.match(runtime.entryPath, /entry\.cjs$/u);
    assert.match(runtime.addonPath, /cua_ax\.node$/u);
    assert.equal(runtime.commandEnv.ELECTRON_RUN_AS_NODE, "1");
  } finally {
    await rm(resourcesPath, { recursive: true, force: true });
  }
});
