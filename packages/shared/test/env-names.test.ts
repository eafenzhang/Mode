import assert from "node:assert/strict";
import test from "node:test";
import { readExternalEnvVar, writeExternalEnvVar } from "../src/env-names.js";

test("读：TUTTO_ 优先，MODE_ 次之，ZCODIUM_、ZCODE_ 兜底", () => {
  const all = {
    TUTTO_DATA_BASE_DIR: "/tutto",
    MODE_DATA_BASE_DIR: "/mode",
    ZCODIUM_DATA_BASE_DIR: "/mode",
    ZCODE_DATA_BASE_DIR: "/mode",
  };
  assert.equal(readExternalEnvVar(all, "MODE_DATA_BASE_DIR"), "/tutto");
  assert.equal(readExternalEnvVar({ MODE_DATA_BASE_DIR: "/mode" }, "MODE_DATA_BASE_DIR"), "/mode");

  const midAndLegacy = { ZCODIUM_DATA_BASE_DIR: "/mode", ZCODE_DATA_BASE_DIR: "/mode" };
  assert.equal(readExternalEnvVar(midAndLegacy, "MODE_DATA_BASE_DIR"), "/mode");

  const onlyLegacy = { ZCODE_DATA_BASE_DIR: "/mode" };
  assert.equal(readExternalEnvVar(onlyLegacy, "MODE_DATA_BASE_DIR"), "/mode");

  const none = {};
  assert.equal(readExternalEnvVar(none, "MODE_DATA_BASE_DIR"), undefined);
});

test("写：TUTTO_ 与三代旧名全写；undefined 全删", () => {
  const env: Record<string, string | undefined> = {};
  writeExternalEnvVar(env, "MODE_DATA_BASE_DIR", "/isolated");
  assert.equal(env.TUTTO_DATA_BASE_DIR, "/isolated");
  assert.equal(env.MODE_DATA_BASE_DIR, "/isolated");
  assert.equal(env.ZCODIUM_DATA_BASE_DIR, "/isolated");
  assert.equal(env.ZCODE_DATA_BASE_DIR, "/isolated");

  writeExternalEnvVar(env, "MODE_DATA_BASE_DIR", undefined);
  assert.equal(env.TUTTO_DATA_BASE_DIR, undefined);
  assert.equal(env.MODE_DATA_BASE_DIR, undefined);
  assert.equal(env.ZCODIUM_DATA_BASE_DIR, undefined);
  assert.equal(env.ZCODE_DATA_BASE_DIR, undefined);
});

test("不在改名名单的变量原样直读", () => {
  const env = { MODE_INTERNAL_ONLY: "/x" };
  assert.equal(readExternalEnvVar(env, "MODE_INTERNAL_ONLY"), "/x");
});
