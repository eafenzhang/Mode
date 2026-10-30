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

// v2 账号体系（docs/specs/cloud-personal-accounts.md）：登录只有 GitHub Device Flow，
// 未登录时自动发起（Host 打开浏览器 + 自动轮询）；目录直连不再有配对码输入。
test("云分区只提供 GitHub 登录，目录设备用连接按钮直连", () => {
  const section = read("../src/remote-desktop/RemoteControlCloudSection.tsx");
  assert.match(
    section,
    /remote-control-cloud-github-login/,
    "远控云分区提供 GitHub 登录按钮",
  );
  assert.doesNotMatch(
    section,
    /remote-control-cloud-password-login/,
    "密码登录入口已删除（v2 仅 GitHub）",
  );
  assert.doesNotMatch(section, /pairDevice\(/, "6 位码配对已由目录直连替代");
  assert.match(section, /remote-control-cloud-connect/, "目录设备行提供连接按钮");
  const dialog = read("../src/ModeAccountDialog.tsx");
  assert.match(
    dialog,
    /modeAccount\.description\.unavailable/,
    "未连接控制面时文案需指向部署缺失，而不是笼统提示",
  );
});
