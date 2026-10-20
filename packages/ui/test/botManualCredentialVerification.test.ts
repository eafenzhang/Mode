import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

// 手填凭据的在线验证（docs/specs/im-bot-onboarding.md R3-5）：
// testBot 鉴权失败时返回 { ok: false, message }，不会抛错。UI 必须读 ok，
// 否则错误凭据会被提示成"已保存并连接"。packages/ui 没有 DOM 环境，
// 沿用本目录的源码契约测试模式。

const dialogSource = await readFile(new URL("../src/BotsDialog.tsx", import.meta.url), "utf8");

function windowAfter(source: string, marker: string, lineSpan: number): string {
  const index = source.indexOf(marker);
  assert.ok(index >= 0, `未找到锚点：${marker}`);
  return source.slice(index).split("\n").slice(0, lineSpan).join("\n");
}

test("verifyManualCredential 读取 testBot 的 ok，失败时给 warning 而不是成功提示", () => {
  const verifier = windowAfter(dialogSource, "const verifyManualCredential = useCallback(", 40);
  const failureIndex = verifier.indexOf("if (!result.ok)");
  assert.ok(
    verifier.includes("const result = await botsService.testBot(saved.id)"),
    "必须接住 testBot 的返回值并检查 ok",
  );
  assert.ok(failureIndex >= 0, "testBot.ok=false 必须走失败分支");
  assert.ok(
    verifier.includes("bots.credentialsVerifyFailed") && verifier.includes('variant: "warning"'),
    "失败分支必须提示凭据验证未通过（warning）",
  );

  // disabled 分支在文件更早处也有一条成功提示；这里只比较失败判断之后的成功提示。
  const successIndex = verifier.indexOf(
    "toast(intl.formatMessage({ id: successMessageId }));",
    failureIndex,
  );
  const failureToastIndex = verifier.indexOf("bots.credentialsVerifyFailed", failureIndex);
  assert.ok(successIndex > failureIndex, "成功提示必须排在 ok 判断之后");
  assert.ok(
    failureToastIndex > failureIndex && failureToastIndex < successIndex,
    "失败提示必须在成功提示之前返回，不能两条都弹",
  );
});

test("停用的机器人仍然跳过在线验证（保存提示照旧）", () => {
  const verifier = windowAfter(dialogSource, "const verifyManualCredential = useCallback(", 40);
  assert.ok(verifier.includes("if (!saved.enabled)"), "enabled=false 不应触发鉴权探测");
  assert.ok(
    verifier.trim().length > 0 && verifier.includes("successMessageId"),
    "停用路径保留成功提示",
  );
});
