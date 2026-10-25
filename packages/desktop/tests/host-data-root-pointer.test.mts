/**
 * 数据根引导指针（默认根 setting.json 的 dataBaseDir）纯函数用例。
 * spec：`docs/specs/tauri-renderer-host.md` §23——数据目录切换的重启生效。
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { join } from "node:path";

import {
  defaultRootSettingsFile,
  persistedDataBaseDirFromSettings,
  withPersistedDataBaseDir,
} from "../src/host-standalone/dataRoot.js";

test("defaultRootSettingsFile 固定落在默认根 .mode/v2", () => {
  assert.equal(
    defaultRootSettingsFile("C:/Users/u"),
    join("C:/Users/u", ".mode", "v2", "setting.json"),
  );
});

test("指针解析：合法 JSON 且 dataBaseDir 非空字符串才认", () => {
  assert.equal(persistedDataBaseDirFromSettings('{"dataBaseDir":"D:/mode-data"}'), "D:/mode-data");
  assert.equal(persistedDataBaseDirFromSettings('{"dataBaseDir":"   "}{'), undefined);
  assert.equal(persistedDataBaseDirFromSettings('{"dataBaseDir":123}'), undefined);
  assert.equal(persistedDataBaseDirFromSettings("{}"), undefined);
});

test("指针解析：坏 JSON / 空 / null 一律视为无指针（回落默认根）", () => {
  assert.equal(persistedDataBaseDirFromSettings("{:wq}"), undefined);
  assert.equal(persistedDataBaseDirFromSettings(""), undefined);
  assert.equal(persistedDataBaseDirFromSettings(null), undefined);
  assert.equal(persistedDataBaseDirFromSettings(undefined), undefined);
});

test("指针值首尾空白被裁剪", () => {
  assert.equal(persistedDataBaseDirFromSettings('{"dataBaseDir":" D:/x "}'), "D:/x");
});

test("withPersistedDataBaseDir：无 env 覆写（home 档）时改写到指针基目录", () => {
  const base = {
    dataBaseDir: "C:/Users/u",
    dataRoot: join("C:/Users/u", ".mode"),
    homeDir: "C:/Users/u",
    source: "home" as const,
  };
  const next = withPersistedDataBaseDir(base, "D:/mode-data");
  assert.equal(next.dataBaseDir, "D:/mode-data");
  assert.equal(next.dataRoot, join("D:/mode-data", ".mode"));
  assert.equal(next.source, "home-pointer");
  assert.equal(next.homeDir, "C:/Users/u");
});

test("withPersistedDataBaseDir：env 覆写档优先级不变，指针被忽略", () => {
  const base = {
    dataBaseDir: "E:/env-override",
    dataRoot: join("E:/env-override", ".mode"),
    homeDir: "C:/Users/u",
    source: "MODE_DATA_BASE_DIR" as const,
  };
  const next = withPersistedDataBaseDir(base, "D:/mode-data");
  assert.equal(next.source, "MODE_DATA_BASE_DIR");
  assert.equal(next.dataBaseDir, "E:/env-override");
});

test("withPersistedDataBaseDir：无指针原样返回", () => {
  const base = {
    dataBaseDir: "C:/Users/u",
    dataRoot: "C:/Users/u/.mode",
    homeDir: "C:/Users/u",
    source: "home" as const,
  };
  assert.equal(withPersistedDataBaseDir(base, undefined), base);
});
