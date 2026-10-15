/**
 * `rd/*` IPC 面（main 侧注册）。
 *
 * 命名与形状由 `IPlatformService` 的 rd* 声明复刻到 preload/renderer；改名字必须四处同步
 * （main 常量 → shared platform → preload → renderer 桥）。
 *
 * 系统 RDP 方案下应用不是会话端点：这里只有「探对端、发起、看会话、断会话、被控设置」
 * 五类，画面/输入/剪贴板/音频没有任何通道（docs/specs/remote-desktop-system-rdp.md 接口节）。
 */
export const REMOTE_DESKTOP_IPC = {
  /** invoke(serverId) → { online, remoteDesktop, systemRdp?, name? }（经 host 用已存令牌探对端 server-info）。 */
  GetPeerCapability: "rd/get-peer-capability",
  /** invoke(RdpConnectRequest) → RdpConnectResult（生成 .rdp 并启动 mstsc）。 */
  Connect: "rd/connect",
  /** invoke() → RdpSessionRecord[]（仅本进程启动的会话）。 */
  GetSessions: "rd/get-sessions",
  /** invoke(sessionId) → { ok }（终止该会话的 mstsc 进程）。 */
  Disconnect: "rd/disconnect",
  /** invoke(serverId) → RdpConnectionOptions（每设备连接选项，按 serverId 记忆）。 */
  GetRdpOptions: "rd/get-rdp-options",
  /** invoke(serverId, options) → RdpConnectionOptions（写入后回读最新值）。 */
  SetRdpOptions: "rd/set-rdp-options",
  /** invoke() → { sharingEnabled, systemRdp? }（被控设置面）。 */
  GetServerState: "rd/get-server-state",
  /** invoke(enabled) → { sharingEnabled, systemRdp? }。 */
  SetServerEnabled: "rd/set-server-enabled",
} as const;
