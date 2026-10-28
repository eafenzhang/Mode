import assert from "node:assert/strict";
import test from "node:test";

import { classifyLicenseExpression } from "../lib/license-classification.mjs";

// 回归：AND 必须取最严格成员，OR 取最宽松成员。
// 旧实现取首个 AND 成员，MIT AND GPL-3.0 会被误判为 green。
test("AND 取最严格成员", () => {
  assert.equal(classifyLicenseExpression("MIT AND GPL-3.0"), "red-gpl");
  assert.equal(classifyLicenseExpression("MIT AND Apache-2.0"), "green");
  assert.equal(classifyLicenseExpression("AGPL-3.0 AND MIT"), "red-agpl");
  assert.equal(classifyLicenseExpression("LGPL-2.1-only AND MIT"), "yellow-lgpl");
  assert.equal(classifyLicenseExpression("MPL-2.0 AND MIT"), "yellow-weak");
});

test("OR 取最宽松成员", () => {
  assert.equal(classifyLicenseExpression("MIT OR GPL-3.0"), "green");
  assert.equal(classifyLicenseExpression("GPL-3.0 OR MIT"), "green");
  assert.equal(classifyLicenseExpression("GPL-3.0 OR AGPL-3.0"), "red-gpl");
});

test("括号递归计算 AND/OR 优先级", () => {
  assert.equal(classifyLicenseExpression("MIT AND (GPL-3.0 OR Apache-2.0)"), "green");
  assert.equal(classifyLicenseExpression("GPL-3.0 AND (MIT OR Apache-2.0)"), "red-gpl");
  assert.equal(classifyLicenseExpression("(MIT OR GPL-3.0) AND Apache-2.0"), "green");
  assert.equal(classifyLicenseExpression("((MIT))"), "green");
});

test("WITH 例外按基础许可证分类", () => {
  assert.equal(classifyLicenseExpression("GPL-2.0-only WITH Classpath-exception-2.0"), "red-gpl");
  assert.equal(classifyLicenseExpression("MIT WITH Some-exception"), "green");
});

test("缺失和未知表达式不会被放行", () => {
  assert.equal(classifyLicenseExpression(""), "unresolved");
  assert.equal(classifyLicenseExpression("(missing)"), "unresolved");
  assert.equal(classifyLicenseExpression("UNLICENSED"), "unresolved");
  assert.equal(classifyLicenseExpression("SEE LICENSE IN LICENSE"), "unresolved");
  assert.equal(classifyLicenseExpression("UNKNOWN"), "unresolved");
  assert.equal(classifyLicenseExpression("FooLicense"), "review");
  assert.equal(classifyLicenseExpression("MIT AND FooLicense"), "review");
  assert.equal(classifyLicenseExpression("MIT AND UNLICENSED"), "unresolved");
});

test("数组声明按 OR 语义合并", () => {
  assert.equal(classifyLicenseExpression(["MIT", "GPL-3.0"]), "green");
  assert.equal(classifyLicenseExpression(["GPL-3.0", "AGPL-3.0"]), "red-gpl");
});
