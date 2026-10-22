import assert from "node:assert/strict";
import test from "node:test";
import { normalizeSettingsPatch } from "../src/setting/normalizeSettingsPatch.js";

// 昵称是本机展示覆盖（docs/specs/user-profile-display.md）：
// 保存时要裁掉首尾空白，空串要归一成 undefined 才能删除旧覆盖。
test("用户昵称保存前裁剪首尾空白", () => {
  assert.equal(normalizeSettingsPatch({ userNickname: "  Alice  " }).userNickname, "Alice");
});

test("用户昵称为空表示删除覆盖", () => {
  assert.equal(normalizeSettingsPatch({ userNickname: "   " }).userNickname, undefined);
});
