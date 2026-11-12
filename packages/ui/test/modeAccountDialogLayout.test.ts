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

// v3 账号体系（docs/specs/remote-control-v3.md）：GitHub OAuth 回调登录（Device Flow 回退），
// 登录后本机作为带 X25519 身份的 Host 上线；设备目录只读，不含业务令牌。
test("v3 账号分区只提供 GitHub 登录与账号状态", () => {
  const section = read("../src/account/RemoteV3AccountSection.tsx");
  assert.match(section, /mode-account-remote-v3-github-login/, "v3 账号分区提供 GitHub 登录按钮");
  assert.doesNotMatch(section, /password-login/, "v3 不提供密码登录");
  assert.doesNotMatch(section, /pairDevice\(/, "v3 不提供配对码输入");
  assert.doesNotMatch(section, /mode-account-remote-v3-devices/, "账号分区不再展示设备列表");
  assert.doesNotMatch(section, /RemoteV3PairingPanel/, "账号分区不提供配对入口");
  assert.match(section, /beginGithubLogin/, "登录必须走 GitHub 账号登录");
  assert.match(section, /githubOauthTitle/, "OAuth 回调登录不展示需要手抄的验证码标题");
  const dialog = read("../src/ModeAccountDialog.tsx");
  assert.match(dialog, /RemoteV3AccountSection/, "账号弹窗必须渲染 v3 登录面");
});
