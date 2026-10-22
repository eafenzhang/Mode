import assert from "node:assert/strict";
import test from "node:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { DATA_ROOT_MANIFEST_FILE_NAME, DATA_ROOT_PRODUCT_ID } from "@mode/shared";

import { initializeDataRootNonInteractive } from "../src/data-root/initializer.js";
import { callWithTimeout, startProductionHost } from "./fixtures/hostStdioClient.js";

// spec: docs/specs/mode-data-root.md「迁移与备份规则」+「Tauri Host 线的现状」。
//
// 演练完整链路：合成旧 `.zcodium` → 初始化器按 migrate 复制（桌面引导/CLI 的职责）→
// **生产 Host** 消费迁移结果。Host 自己不做数据根初始化（缺口已登记），所以这里显式先迁移。

const delay = (ms: number): Promise<void> =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

function sandbox(): { root: string; base: string; legacyRoot: string } {
  const root = mkdtempSync(join(tmpdir(), "mode-data-root-drill-"));
  const base = join(root, "data");
  mkdirSync(base, { recursive: true });
  return { root, base, legacyRoot: join(base, ".zcodium") };
}

async function removeSandbox(root: string): Promise<void> {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    try {
      rmSync(root, { recursive: true, force: true });
      return;
    } catch (error) {
      if (attempt === 9) {
        process.stderr.write(`[data-root-drill] 沙箱清理失败（不影响断言）：${String(error)}\n`);
        return;
      }
      await delay(150);
    }
  }
}

test("迁移演练：旧根复制到新根后，生产 Host 读到旧设置、写入落新根、旧根不变", async () => {
  const box = sandbox();
  const legacySetting = join(box.legacyRoot, "v2", "setting.json");
  const legacyDb = join(box.legacyRoot, "cli", "db", "db.sqlite");
  mkdirSync(dirname(legacySetting), { recursive: true });
  mkdirSync(dirname(legacyDb), { recursive: true });
  writeFileSync(legacySetting, JSON.stringify({ locale: "en-US" }));
  writeFileSync(legacyDb, "legacy-db-bytes");

  // 顺序是语义：迁移必须在 Host 启动**之前**完成（桌面引导 / CLI 的职责）。
  // 先起 Host 会让服务隐式创建无归属的 `.mode`，再迁移就变成 unowned 备份让路。
  const initialized = await initializeDataRootNonInteractive({
    baseDir: box.base,
    createdBy: "desktop",
    appVersion: "0.0.0-test",
    action: "migrate",
  });
  assert.equal(initialized.state, "initialized");

  const host = await startProductionHost({ home: box.base, dataBaseDir: box.base });
  try {
    const dataRoot = join(box.base, ".mode");
    const manifest = JSON.parse(
      readFileSync(join(dataRoot, DATA_ROOT_MANIFEST_FILE_NAME), "utf8"),
    ) as {
      product?: string;
      migration?: { mode?: string; from?: string };
    };
    assert.equal(manifest.product, DATA_ROOT_PRODUCT_ID);
    assert.equal(manifest.migration?.mode, "copy");
    assert.equal(manifest.migration?.from, box.legacyRoot);
    assert.equal(
      readFileSync(join(dataRoot, "cli", "db", "db.sqlite"), "utf8"),
      "legacy-db-bytes",
      "旧数据必须逐字节复制到新根",
    );

    // Host 读迁移后的设置（`en-US` 不是默认值 `zh-CN`，能区分「读到旧数据」与「读到默认值」）。
    const settings = await callWithTimeout(
      host.client.getChannel("setting").call<Record<string, unknown>>("get"),
      "setting.get",
    );
    assert.equal(settings.locale, "en-US", "Host 必须消费迁移后的设置");

    // 新写入落新根；旧根只复制、不删除、不改写。
    await callWithTimeout(
      host.client.getChannel("setting").call("update", [{ locale: "zh-CN" }]),
      "setting.update",
    );
    await delay(200);
    assert.equal(
      (
        JSON.parse(readFileSync(join(dataRoot, "v2", "setting.json"), "utf8")) as {
          locale?: string;
        }
      ).locale,
      "zh-CN",
      "迁移后的写入必须落在新根",
    );
    assert.equal(
      readFileSync(legacySetting, "utf8"),
      JSON.stringify({ locale: "en-US" }),
      "旧根不得被改写（只复制，不删除）",
    );
  } finally {
    await host.stop();
    await removeSandbox(box.root);
  }
});
