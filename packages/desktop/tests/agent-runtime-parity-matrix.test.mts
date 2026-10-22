/**
 * M2 Task 7 对拍矩阵门禁的纯函数用例（spec：
 * `docs/specs/agent-runtime-parity-matrix.md`「CI 门禁」）。
 *
 * 门禁只做两件事：services 测试必须显式归类、运行时对拍资产必须存在。
 * 这里不碰真实文件系统，避免测试随仓库文件增删而漂移；CLI 的目录读取在 main 里。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  PARITY_ASSETS,
  buildParityReport,
  classifyServicesTestFile,
} from "../../../scripts/agent-runtime-parity-matrix.ts";

const ALL_PRESENT = (): boolean => true;

test("services 测试按域归类，未分类返回 null", () => {
  assert.equal(classifyServicesTestFile("bot-dingtalk.test.ts")?.domain, "bot");
  assert.equal(classifyServicesTestFile("cua-helper-admission.test.ts")?.domain, "cua");
  assert.equal(classifyServicesTestFile("dataRootMigration.test.ts")?.domain, "data");
  assert.equal(classifyServicesTestFile("data-root-host-drill.test.ts")?.domain, "data");
  assert.equal(classifyServicesTestFile("plugin-marketplace-defaults.test.ts")?.domain, "plugin");
  assert.equal(classifyServicesTestFile("providerConfigMigration.test.ts")?.domain, "provider");
  assert.equal(
    classifyServicesTestFile("conversationMirrorService.test.ts")?.verdict,
    "not-translatable",
  );
  assert.equal(classifyServicesTestFile("credential-service.test.ts")?.domain, "misc");
  assert.equal(classifyServicesTestFile("settings-user-nickname.test.ts")?.domain, "misc");
  assert.equal(classifyServicesTestFile("brand-new-runtime.test.ts"), null);
});

test("未分类文件让报告失败并列出文件名", () => {
  const report = buildParityReport({
    servicesTestFiles: ["bot-dingtalk.test.ts", "brand-new-runtime.test.ts"],
    assetExists: ALL_PRESENT,
  });

  assert.equal(report.services.total, 2);
  assert.equal(report.services.classified, 1);
  assert.deepEqual(report.services.unclassified, ["brand-new-runtime.test.ts"]);
  assert.equal(report.ok, false);
});

test("对拍资产缺失让报告失败并列出路径", () => {
  const report = buildParityReport({
    servicesTestFiles: ["bot-dingtalk.test.ts"],
    assetExists: (asset) => asset !== "crates/agent-core/tests/tool_parity.rs",
  });

  assert.deepEqual(report.assets.missing, ["crates/agent-core/tests/tool_parity.rs"]);
  assert.equal(report.ok, false);
});

test("全部分类且资产齐全时报告通过，并按域计数", () => {
  const report = buildParityReport({
    servicesTestFiles: [
      "bot-dingtalk.test.ts",
      "bot-heartbeat.test.ts",
      "cua-raw-input.test.ts",
      "dataRootOwnership.test.ts",
      "plugin-marketplace-id-compat.test.ts",
      "providerBalanceParsing.test.ts",
      "conversation-typing-bubble.test.ts",
      "wecom-registration.test.ts",
    ],
    assetExists: ALL_PRESENT,
  });

  assert.equal(report.ok, true);
  assert.equal(report.services.classified, 8);
  assert.equal(report.services.byDomain.bot?.count, 2);
  assert.equal(report.services.byDomain["host-untranslatable"]?.count, 1);
  assert.equal(report.assets.declared.length, PARITY_ASSETS.length);
  assert.deepEqual(report.assets.missing, []);
});
