/**
 * `MODE_DATA_BASE_DIR` 隔离根的首次初始化。
 *
 * 背景（2026-10-31 本地验收复现）：Tauri 壳会先写 `.mode/v2/logs/host-shell.log`，
 * 随后 CLI/agent 才执行数据根初始化。此时 `.mode` 已存在但没有归属文件，默认 fresh
 * 会把整个目录 rename 成 `.mode.unowned-*`；Windows 上有打开句柄时 rename 直接 EPERM，
 * agent 进程退出。隔离 base 应原地补写归属，而不是备份让路。
 *
 * `paths.ts` 在模块加载时捕获 MODE_DATA_BASE_DIR，所以必须先设环境变量再动态 import。
 */
import assert from "node:assert/strict";
import test from "node:test";
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
import { join } from "node:path";
import { DATA_ROOT_MANIFEST_FILE_NAME, DATA_ROOT_PRODUCT_ID } from "@mode/shared";

const BASE = mkdtempSync(join(tmpdir(), "mode-env-data-root-"));
process.env.MODE_DATA_BASE_DIR = BASE;

const init = await import("../src/data-root/initializer.js");

function seedShellCreatedRoot(): string {
  const root = join(BASE, ".mode");
  mkdirSync(join(root, "v2", "logs"), { recursive: true });
  writeFileSync(join(root, "v2", "logs", "host-shell.log"), "shell-before-init\n");
  return root;
}

function assertAdoptedInPlace(root: string): void {
  assert.equal(existsSync(join(root, DATA_ROOT_MANIFEST_FILE_NAME)), true);
  assert.equal(
    readFileSync(join(root, "v2", "logs", "host-shell.log"), "utf8"),
    "shell-before-init\n",
  );
  const backups = readdirSync(BASE).filter((name) => name.startsWith(".mode.unowned-"));
  assert.deepEqual(backups, []);
}

test("interactive：隔离根被壳先写日志时原地补写归属", () => {
  const root = seedShellCreatedRoot();
  try {
    const result = init.initializeDataRootInteractive({
      createdBy: "desktop",
      appVersion: "3.15.0",
    });
    assert.equal(result.state, "initialized");
    assert.equal(result.initializedBy, "fresh");
    const manifest = JSON.parse(readFileSync(join(root, DATA_ROOT_MANIFEST_FILE_NAME), "utf8"));
    assert.equal(manifest.product, DATA_ROOT_PRODUCT_ID);
    assertAdoptedInPlace(root);
  } finally {
    init.resetDataRootInitializerForTest();
    rmSync(root, { recursive: true, force: true });
  }
});

test("nonInteractive：隔离根 fresh 不再备份让路", async () => {
  const root = seedShellCreatedRoot();
  try {
    const result = await init.initializeDataRootNonInteractive({
      createdBy: "cli",
      appVersion: "3.15.0",
      action: "fresh",
    });
    assert.equal(result.state, "initialized");
    assert.equal(result.initializedBy, "fresh");
    assert.equal("forfeitedRoot" in result, false);
    assertAdoptedInPlace(root);
  } finally {
    init.resetDataRootInitializerForTest();
    rmSync(root, { recursive: true, force: true });
    rmSync(BASE, { recursive: true, force: true });
  }
});
