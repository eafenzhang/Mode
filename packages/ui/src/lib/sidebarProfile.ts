import type { RemoteV3HostState, UserInfo } from "@mode/shared";

/**
 * 左下角账号区展示模型（docs/specs/user-profile-display.md）。
 *
 * 首页与设置页共用同一解析结果：
 * - 身份只认 OAuth 用户或已登录的远程控制 v3 账号；
 * - 本地微信 Bot 绑定（providerUserId 等）是 bot 通道信息，不是用户资料；
 * - 昵称只做本机展示覆盖，不写回任何账号。
 */
export interface SidebarProfile {
  /** 是否存在可展示的登录身份（OAuth / v3）。 */
  hasAccount: boolean;
  displayName: string;
  avatarUrl?: string;
}

function trimToUndefined(value: string | null | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
}

export function resolveSidebarProfile(input: {
  user?: UserInfo | null;
  remoteV3?: Pick<RemoteV3HostState, "status" | "accountName"> | null;
  nickname?: string | null;
}): SidebarProfile {
  const nickname = trimToUndefined(input.nickname);
  const user = input.user ?? null;
  if (user) {
    const avatarUrl = trimToUndefined(user.avatarUrl);
    return {
      hasAccount: true,
      displayName:
        nickname ?? trimToUndefined(user.displayName) ?? trimToUndefined(user.username) ?? "Mode",
      ...(avatarUrl ? { avatarUrl } : {}),
    };
  }

  const remoteV3 = input.remoteV3 ?? null;
  if (remoteV3?.status === "online" || remoteV3?.status === "reconnecting") {
    return {
      hasAccount: true,
      displayName: nickname ?? trimToUndefined(remoteV3.accountName) ?? "Mode",
    };
  }

  return { hasAccount: false, displayName: "Mode" };
}
