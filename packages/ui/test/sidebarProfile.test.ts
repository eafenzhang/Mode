import assert from "node:assert/strict";
import test from "node:test";
import type { CloudAccountState, UserInfo } from "@mode/shared";
import { resolveSidebarProfile } from "../src/lib/sidebarProfile.js";

// 左下角账号区展示口径（docs/specs/user-profile-display.md）：
// 首页与设置页必须解析出同一份资料，且本地微信 Bot 的 openid 不是用户资料。
const OAUTH_USER: UserInfo = {
  id: "user-1",
  username: "alice",
  displayName: "Alice",
};

function cloudState(overrides: Partial<CloudAccountState>): CloudAccountState {
  return {
    status: "unbound",
    enrollmentConfigured: true,
    ...overrides,
  };
}

test("未登录时显示 Mode，不把品牌名当作用户名", () => {
  const profile = resolveSidebarProfile({ user: null, cloudAccount: null });
  assert.equal(profile.hasAccount, false);
  assert.equal(profile.displayName, "Mode");
});

test("只绑定微信 Bot 时不得回落 providerUserId（o9cg...）", () => {
  // boundWeixinName 可能是 bot 的 providerUserId；footer 之前把它当成显示名，
  // 导致设置页出现「o9cg805lbPlUBF…」而首页显示 Mode。
  const profile = resolveSidebarProfile({
    user: null,
    cloudAccount: cloudState({
      status: "unbound",
      boundWeixinName: "o9cg805lbPlUBFxxxxxxxx",
    }),
  });
  assert.equal(profile.hasAccount, false);
  assert.equal(profile.displayName, "Mode");
});

test("Mode Cloud 已绑定时使用云端账号展示名", () => {
  const profile = resolveSidebarProfile({
    user: null,
    cloudAccount: cloudState({
      status: "bound",
      account: { id: "acct-1", displayName: "Alice Cloud" },
      boundWeixinName: "o9cg805lbPlUBFxxxxxxxx",
    }),
  });
  assert.equal(profile.hasAccount, true);
  assert.equal(profile.displayName, "Alice Cloud");
});

test("云账号没有展示名时保持 Mode，等待昵称覆盖", () => {
  const profile = resolveSidebarProfile({
    user: null,
    cloudAccount: cloudState({ status: "bound", account: { id: "acct-1" } }),
  });
  assert.equal(profile.hasAccount, true);
  assert.equal(profile.displayName, "Mode");
});

test("OAuth 用户展示名优先于 opaque username，并保留头像", () => {
  const profile = resolveSidebarProfile({
    user: { id: "u", username: "o9cg805lbPlUBF", displayName: "Alice", avatarUrl: "https://a" },
  });
  assert.equal(profile.displayName, "Alice");
  assert.equal(profile.avatarUrl, "https://a");
});

test("本地昵称覆盖所有账号名称，空白昵称回退账号名", () => {
  assert.equal(
    resolveSidebarProfile({ user: OAUTH_USER, nickname: "  小 A  " }).displayName,
    "小 A",
  );
  assert.equal(resolveSidebarProfile({ user: OAUTH_USER, nickname: "   " }).displayName, "Alice");
});
