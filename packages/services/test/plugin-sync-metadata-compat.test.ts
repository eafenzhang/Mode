import assert from "node:assert/strict";
import test from "node:test";

import {
  PLUGIN_SYNC_METADATA_ARCHIVE_PATH,
  resolvePluginSyncMetadataArchiveFileName,
} from "../src/plugin-sync/pluginSyncArchive.js";

test("插件同步元数据文件名：新名优先，旧对端归档可读", () => {
  assert.equal(PLUGIN_SYNC_METADATA_ARCHIVE_PATH, ".tutto-plugin-sync.json");
  assert.equal(
    resolvePluginSyncMetadataArchiveFileName((name) => name === ".tutto-plugin-sync.json"),
    ".tutto-plugin-sync.json",
  );
  assert.equal(
    resolvePluginSyncMetadataArchiveFileName((name) => name === ".mode-plugin-sync.json"),
    ".mode-plugin-sync.json",
  );
  assert.equal(
    resolvePluginSyncMetadataArchiveFileName(() => false),
    ".tutto-plugin-sync.json",
  );
});
