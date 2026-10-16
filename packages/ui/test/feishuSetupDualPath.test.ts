import assert from "node:assert/strict";
import test from "node:test";

// 飞书接入的"双路"（docs/specs/im-bot-onboarding.md 验收 2）：无凭据时扫码与手填两个入口同时可用，
// 手填保存后 feishuAppId 与凭据一起落盘，且此后仍走既有"有凭据但未绑定"状态机。
//
// 为什么用源码契约而不是渲染测试：packages/ui 有测试运行器（CI 跑 `pnpm --filter @mode/ui test`），
// 但**没有 DOM 环境**（未引入 jsdom / testing-library），所以只能断言接线，不能断言渲染结果。
const cardSource = await import("node:fs/promises").then((fs) =>
  fs.readFile(new URL("../src/BotsDialog/ProviderSettingsCard.tsx", import.meta.url), "utf8"),
);
const dialogSource = await import("node:fs/promises").then((fs) =>
  fs.readFile(new URL("../src/BotsDialog.tsx", import.meta.url), "utf8"),
);

/** 取 `start` 标记之后的窗口（行窗口，避免跨函数误命中）。 */
function windowAfter(source: string, marker: string, lineSpan: number): string {
  const index = source.indexOf(marker);
  assert.ok(index >= 0, `未找到锚点：${marker}`);
  return source.slice(index).split("\n").slice(0, lineSpan).join("\n");
}

test("飞书面板同时提供扫码与手填两条入口，且都渲染成可点的控件", () => {
  assert.ok(cardSource, "无法读取 ProviderSettingsCard 源码");
  // 局部组件的位置会随文件增长下移，窗口给宽一些；锚点是它的函数声明。
  const panel = windowAfter(cardSource!, "function FeishuSetupPanel(", 160);
  assert.ok(
    panel.includes("onStartRegistration") && panel.includes("onSaveManual"),
    "两条入口必须同时存在于飞书面板的 props 里",
  );
  assert.ok(
    panel.includes("onClick={onStartRegistration}"),
    "扫码入口必须绑定到 onStartRegistration",
  );
  assert.ok(
    panel.includes("onSaveManual(appIdDraft.trim(), credentialValue)"),
    "手填入口必须把 App ID 草稿与 secret 一起交给 onSaveManual",
  );
  assert.ok(
    panel.includes("disabled={!canSaveManual}") && panel.includes("canSaveManual ="),
    "手填按钮必须受「App ID 与 secret 都非空」门控，不能提交半份凭据",
  );
});

test("手填保存：feishuAppId 与凭据在同一次 saveBot 里落盘（单写路径），并收起扫码态", () => {
  assert.ok(dialogSource, "无法读取 BotsDialog 源码");
  const handler = windowAfter(dialogSource!, "const handleSaveFeishuManual = useCallback(", 26);
  assert.ok(handler.includes("saveBot("), "凭据必须经 saveBot 写入");
  assert.ok(
    handler.includes("feishuAppId") && handler.includes("credentialValue: secret"),
    "同一次 saveBot 调用里要同时带上 feishuAppId 与 credentialValue",
  );
  assert.ok(
    !handler.includes("credentialService"),
    "不得绕过 saveBot 直接写 credentialService——R4 只允许一条凭据写路径",
  );
  assert.ok(
    handler.includes("setFeishuRegistration(null)"),
    "手填成功后要收起扫码态：两条路径是同一个「凭据就绪」结果，不该并存呈现",
  );
  // 两条路径收敛到同一个后续状态机：手填与扫码成功都走 verifyManualCredential 的校验+绑定码分支。
  assert.ok(
    handler.includes("verifyManualCredential("),
    "手填保存后必须复用既有的校验/绑定码状态机，而不是另开一条路径",
  );
});
