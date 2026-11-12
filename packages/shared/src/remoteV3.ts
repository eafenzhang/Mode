import type { RemoteDesktopCodec, RemoteDesktopQuality } from "./remote-desktop.js";

/** 远程控制 v3 Host 管理面（docs/specs/remote-control-v3.md）。 */

export type RemoteV3Status = "unconfigured" | "connecting" | "online" | "reconnecting" | "error";

export interface RemoteV3PendingLogin {
  userCode: string;
  verificationUri: string;
  expiresAt: number;
}

export interface RemoteV3HostState {
  status: RemoteV3Status;
  serverUrl: string | null;
  /** 账号服务返回的展示名（GitHub login/name），仅用于本机展示。 */
  accountName: string | null;
  deviceId: string | null;
  deviceName: string | null;
  fingerprint: string | null;
  error: string | null;
  activeConnections: number;
  pendingLogin: RemoteV3PendingLogin | null;
}

export interface RemoteV3ConfigureParams {
  serverUrl: string;
  /** 账号服务 Web 会话 token；由 GitHub Device Flow 或运维流程获得。 */
  webSessionToken: string;
  deviceName?: string;
}

export interface RemoteV3BeginLoginParams {
  serverUrl?: string;
}

export interface RemoteV3DeviceSummary {
  deviceId: string;
  name: string;
  role: "host" | "client";
  platform: string;
  identityKey: string;
  online: boolean;
  addresses: string[];
}

export interface RemoteV3GithubChallenge {
  userCode: string;
  verificationUri: string;
  expiresIn: number;
  interval: number;
}

export interface RemoteV3ResetPinParams {
  deviceId: string;
}

export interface RemoteV3RequestParams {
  hostDeviceId: string;
  method: string;
  params?: Record<string, unknown>;
  timeoutMs?: number;
}

export interface RemoteV3RequestResult {
  result: unknown;
}

export interface RemoteV3TunnelOpenParams {
  hostDeviceId: string;
  /** 目标 Host 本机 LAN 服务的 WebSocket 路径（必须 `/ws` 前缀，可带查询参数）。 */
  path: string;
  /** 附加握手头（如 x-mode-rpc-host-capability）；Cookie/Authorization 不允许覆盖。 */
  headers?: Record<string, string>;
}

export interface RemoteV3HttpRequestParams {
  hostDeviceId: string;
  method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
  /** 目标 Host 本机 `/api/*` 路径；`/api/lan/*` 与 `/api/remote-agent/*` 被拒绝。 */
  path: string;
  headers?: Record<string, string>;
  bodyBase64?: string;
  timeoutMs?: number;
}

export interface RemoteV3HttpResponse {
  status: number;
  headers: Record<string, string>;
  bodyBase64: string;
}

export interface RemoteV3RemoteDesktopStartParams {
  hostDeviceId: string;
  quality?: RemoteDesktopQuality;
  codec?: RemoteDesktopCodec;
}

export interface RemoteV3RemoteDesktopSession {
  /** 本机回环信令 WS；现有 controller 直接连接。 */
  signalUrl: string;
  /** 本机回环媒体 WS；现有 renderer 媒体客户端直接连接。 */
  mediaUrl: string;
}

export interface RemoteV3TunnelEvent {
  tunnelId: string;
  kind: "open" | "message" | "close";
  /** base64url；首字节 0=text、1=binary，其余为原始帧。 */
  data?: string;
  reason?: string;
}

/**
 * GitHub Device Flow 授权地址：带上 `user_code`，GitHub 已登录时可直接确认，
 * 不再要求用户手动抄写验证码。GitHub 忽略该参数时页面仍可手动输入。
 */
export function buildGithubDeviceAuthorizationUrl(
  verificationUri: string,
  userCode: string,
): string {
  try {
    const url = new URL(verificationUri);
    if (!url.searchParams.has("user_code")) url.searchParams.set("user_code", userCode);
    return url.toString();
  } catch {
    return verificationUri;
  }
}
