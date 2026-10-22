import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (relativePath: string): string =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

// M4 Dialog 适配器只给 shell（圆角/边框/阴影），内边距由调用方提供；
// 漏传 p-* 时标题与内容会直接压在圆角边框上（2026-10-21 Mode 账号弹窗真机截图）。
test("ModeAccountDialog 自带内边距与纵向间距", () => {
  const source = read("../src/ModeAccountDialog.tsx");
  assert.match(
    source,
    /flex max-w-2xl flex-col gap-4 p-4/,
    "账号弹窗 shell 必须有 p-4 + 纵向 gap，避免内容贴着圆角边缘",
  );
});

test("ModeAccountDialog 标题区给右上角关闭按钮留位", () => {
  const source = read("../src/ModeAccountDialog.tsx");
  assert.match(source, /<DialogHeader className="pe-10">/, "header 需预留关闭按钮空间");
});

// 昵称入口必须在 Mode 账号弹窗内，且写的是本机 AppSettings.userNickname，
// 不能把微信 bot 的 openid 当作用户展示名（docs/specs/user-profile-display.md）。
test("ModeAccountDialog 提供昵称编辑并写入 userNickname", () => {
  const source = read("../src/ModeAccountDialog.tsx");
  assert.match(source, /mode-account-nickname-input/, "弹窗需有昵称输入框");
  assert.match(source, /update\(\{ userNickname:/, "保存动作需写入 AppSettings.userNickname");
  assert.match(
    source,
    /resolveSidebarProfile\(\{/,
    "是否已登录 / 显示名必须复用统一解析，不能在弹窗内另写一套",
  );
});

// 没有部署密钥不等于云账号不可用：微信 6 位码登录不依赖 enrollment secret，
// 未配置时也必须保留入口（docs/specs/mode-cloud-account-directory.md）。
test("未配置部署密钥时仍保留微信登录入口", () => {
  const section = read("../src/remote-desktop/RemoteControlCloudSection.tsx");
  assert.match(
    section,
    /remote-control-cloud-wechat-login/,
    "远控云分区始终提供微信登录按钮",
  );
  const dialog = read("../src/ModeAccountDialog.tsx");
  assert.match(
    dialog,
    /modeAccount\.description\.wechatOnly/,
    "未配置部署密钥时文案需指向微信登录，而不是笼统提示未启用",
  );
});
