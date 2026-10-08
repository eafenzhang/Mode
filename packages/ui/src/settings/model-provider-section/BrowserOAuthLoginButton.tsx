import type { OAuthProviderId } from "@mode/shared";

/**
 * 浏览器授权登录入口。
 *
 * 官方账号体系已随 Z.AI 服务一并下线：OAuth 流程在服务层被拒绝（`assertOfficialServiceRemoved("account")`），
 * 没有开关可以恢复。这里保留空实现让调用点无需分支，模型供应商仍可通过 API Key 正常配置使用。
 */
export function BrowserOAuthLoginButton(_props: {
  oauthProviderId: OAuthProviderId;
  providerName: string;
  disabled?: boolean;
}) {
  return null;
}
