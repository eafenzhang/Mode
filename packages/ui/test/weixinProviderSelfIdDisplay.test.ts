import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

// 微信 providerSelfId 的 UI 门槛（docs/specs/weixin-provider-self-id.md 规则 8）：
// 扫码成功写 providerSelfId（不再写 providerUserId）；摘要卡已绑定态认
// providerSelfId，否则迁移后微信注册完显示"未绑定"。UI 组件不引服务层，
// 沿用本目录源码契约模式（botBindingMenuDisplay 同类）。

const dialogsSource = await readFile(new URL("../src/BotsDialog.tsx", import.meta.url), "utf8");
const summarySource = await readFile(
  new URL("../src/BotsDialog/BotSummaryCard.tsx", import.meta.url),
  "utf8",
);

test("微信扫码成功：写 providerSelfId，不再写 providerUserId", () => {
  const successIndex = dialogsSource.indexOf("providerSelfId:");
  assert.ok(successIndex >= 0, "扫码成功分支必须写 providerSelfId");
  // 同一分支（saveBot 的对象字面量）内不得再出现 providerUserId 赋值
  const literalStart = dialogsSource.lastIndexOf("{", successIndex);
  const literalEnd = dialogsSource.indexOf("}", successIndex);
  const registrationLiteral = dialogsSource.slice(literalStart, literalEnd);
  assert.ok(
    !registrationLiteral.includes("providerUserId:"),
    `扫码成功不再写 providerUserId（注册双写是幻影对话根因），实际字面量：${registrationLiteral}`,
  );
});

test("BotSummaryCard 已绑定态认 providerSelfId（微信迁移后不显示未绑定）", () => {
  assert.ok(summarySource.includes("providerSelfId"), "isBound 必须包含 providerSelfId 判定");
});
