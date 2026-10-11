/**
 * `rd/*` IPC 面（main 侧注册；renderer/preload 封装由 T5 接）。
 *
 * 命名与形状**冻结**：T5 的 `IPlatformService` 与缓存客户端按这里的字符串消费，不要重命名。
 * 仅新增通道可扩；现有通道的请求/响应字段不得改。
 */
export const REMOTE_DESKTOP_IPC = {
  /** invoke(serverId) → { remoteDesktop, online, name? }（经 host 用已存令牌探对端 server-info）。 */
  GetPeerCapability: "rd/get-peer-capability",
  /** invoke(serverId) → { ok: true } | { error: RemoteDesktopErrorCode }。 */
  StartSession: "rd/start-session",
  /** invoke(message: RemoteDesktopSignalMessage)（main 校验后写出）。 */
  SendSignal: "rd/send-signal",
  /** invoke(reason?)（先发 bye 再收尾）。 */
  EndSession: "rd/end-session",
  /** invoke() → { enabled, authorizedClients }（被控设置面）。 */
  GetServerState: "rd/get-server-state",
  /** invoke(enabled) → { enabled, authorizedClients }。 */
  SetServerEnabled: "rd/set-server-enabled",
  /** invoke(clientId) → { enabled, authorizedClients }。 */
  RevokeClient: "rd/revoke-client",
  /** push（main → renderer）：{ kind:"state", state, error? } | { kind:"signal", message }。 */
  SessionEvent: "rd/session-event",
} as const;
