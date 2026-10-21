import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (relativePath: string): string =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

test("SettingsRow 控制列允许换行，避免固定列宽时向右溢出", () => {
  const source = read("../src/settings/SettingsPageParts.tsx");
  assert.match(
    source,
    /flex w-full min-w-0 flex-wrap items-center justify-end gap-2/,
    "inline 控制列必须有 min-w-0 + flex-wrap（BotsDialog 右栏横向溢出的根因）",
  );
});

test("BotsDialog 右栏禁止横向滚动，纵向滚动保持", () => {
  const source = read("../src/BotsDialog.tsx");
  assert.match(source, /overflow-x-hidden overflow-y-auto/);
});

test("Bot 详情设置使用 inline 布局，控件在右列", () => {
  const settingsRow = read("../src/settings/SettingsPageParts.tsx");
  assert.ok(!settingsRow.includes('layout === "stacked"'), "SettingsRow 不再保留 stacked 分支");
  assert.match(
    settingsRow,
    /justify-end gap-2/,
    "inline 布局的控制列必须右对齐（标题/说明在左、控件在右）",
  );

  for (const file of [
    "../src/BotsDialog/BotHeartbeatCard.tsx",
    "../src/BotsDialog/BotDeliveryModeCard.tsx",
    "../src/BotsDialog/BotPrivateChatCard.tsx",
    "../src/BotsDialog/BotGroupChatCard.tsx",
    "../src/BotsDialog/WorkspaceManagementCard.tsx",
    "../src/BotsDialog/BotDingtalkCardConfigCard.tsx",
    "../src/BotsDialog/BotSummaryCard.tsx",
    "../src/BotsDialog/ProviderSettingsCard.tsx",
  ]) {
    assert.ok(!read(file).includes('layout="stacked"'), `${file} 不应再使用 stacked 布局`);
  }
});

test("BotsDialog 内容区有统一内边距，次要文字局部提亮", () => {
  const source = read("../src/BotsDialog.tsx");
  assert.match(
    source,
    /flex-col gap-3 overflow-hidden rounded-2xl p-4/,
    "弹窗 shell 必须有 p-4 + gap-3，避免内容贴着圆角边缘",
  );
  assert.match(source, /--color-foreground-subtle/, "次要文字对比度应在弹窗范围内覆盖");
});

test("Bot 名称使用前景色，并提供明确的编辑入口", () => {
  const source = read("../src/BotsDialog/BotSummaryCard.tsx");
  assert.match(
    source,
    /text-ui-lg font-medium text-foreground/,
    "摘要名称必须显式使用前景色，不能继承弹窗的次要文字颜色",
  );
  assert.match(source, /bots\.rename/, "名称编辑入口需要可读的 title/aria-label");
  assert.match(source, /<Pencil/, "名称旁需要铅笔图标提示可重命名");
  assert.match(
    source,
    /bg-surface px-1 py-0\.5[^"]*ring-1 ring-border/,
    "进入重命名时输入框要有可见的底色和边框",
  );
});

test("机器人心跳数字左对齐显示", () => {
  const source = read("../src/BotsDialog/BotHeartbeatCard.tsx");
  assert.match(source, /pe-11 text-start/, "心跳输入框数值必须左对齐");
  assert.ok(!source.includes("text-end"), "心跳输入框不再右对齐数值");
});

test("三段状态行放在左列说明下，不占用右侧操作列", () => {
  const source = read("../src/BotsDialog/ProviderSettingsCard.tsx");
  assert.match(source, /justify-start/, "状态行在左列内左对齐");
  assert.match(source, /description=\{descriptionNode\}/, "说明列必须渲染状态行");
  assert.equal(
    (source.match(/\{stageRow\}/gu) ?? []).length,
    1,
    "状态行只应在左侧说明列渲染一次，右侧控制列不再重复",
  );
});

test("Bot 详情把设置合并为三组，并移除独立 Agent 卡", () => {
  const source = read("../src/BotsDialog.tsx");
  assert.equal(
    (source.match(/<SettingsGroupCard>/gu) ?? []).length >= 3,
    true,
    "至少按自动回复 / 聊天接入 / 工作区三组收口",
  );
  assert.ok(!source.includes("<BotAgentOwnershipCard"), "Agent 归属已并入工作区管理");
});
