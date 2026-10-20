import assert from "node:assert/strict";
import { resolve } from "node:path";
import test from "node:test";

import {
  applyHostResourcesPath,
  HOST_RESOURCES_DIR_ENV,
  resolveHostResourcesPath,
} from "../src/host-standalone/runtimeResources.js";

/**
 * Tauri Host 是纯 Node sidecar：没有 Electron 注入的 `process.resourcesPath`。
 * 这条桥接缺失的实测症状是 `rd_media_start` 固定报
 * `Computer Use Helper is not reachable for rd_media`，主控显示「对端不支持远程桌面」。
 */
test("resourcesPath：空字段时从 MODE_HOST_RESOURCES_DIR 桥接绝对路径", () => {
  const host: { resourcesPath?: string } = {};
  const resources = resolve("resources");
  const applied = applyHostResourcesPath({ [HOST_RESOURCES_DIR_ENV]: resources }, host);
  assert.equal(applied, resources);
  assert.equal(host.resourcesPath, resources);
});

test("resourcesPath：已由宿主注入时保持原值，不被环境变量覆盖", () => {
  const host = { resourcesPath: resolve("electron-resources") };
  const applied = applyHostResourcesPath(
    { [HOST_RESOURCES_DIR_ENV]: resolve("tauri-resources") },
    host,
  );
  assert.equal(applied, host.resourcesPath);
});

test("resourcesPath：相对路径/空值不采用，保持 Node 原状", () => {
  const relative: { resourcesPath?: string } = {};
  assert.equal(
    resolveHostResourcesPath({ [HOST_RESOURCES_DIR_ENV]: "relative/resources" }),
    undefined,
  );
  assert.equal(
    applyHostResourcesPath({ [HOST_RESOURCES_DIR_ENV]: "relative/resources" }, relative),
    undefined,
  );
  assert.equal(relative.resourcesPath, undefined);

  const blank: { resourcesPath?: string } = {};
  assert.equal(applyHostResourcesPath({ [HOST_RESOURCES_DIR_ENV]: "   " }, blank), undefined);
  assert.equal(blank.resourcesPath, undefined);
});
