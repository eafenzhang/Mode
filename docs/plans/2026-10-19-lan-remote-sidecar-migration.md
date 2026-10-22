# LAN / 远控迁移到 Tauri Host sidecar（独立立项）

状态：2026-10-19 立项；LR-0..LR-4 的代码与自动化面已随 `bd4c7e46` 落地；LR-5 未开工。
2026-10-20 的 0.0.61 真机验收：LAN 监听、配对码、发现与已配对设备通过；远程桌面因对端缺
`mediaTicket` 返回「对端不支持」；远程工作区在设置页仍无 attach/打开入口；两台实机端到端
验收未完成。SSH/WSL/Docker 已接 `connectStdio`（自动化覆盖，真机未验）；bot reconnect 已接通
（2026-10-22，Host 直连传输 + 进程内 runtime port + `botReconnected` 事件）；UI tab 降级仍待做。
同日复现更正：`mediaTicket` 并不是根因，真正断点是 Tauri 包未带 Windows CUA helper
（`rd_media_start` → `helper_unavailable` → 对端回 `unsupported`）；修复与复现口径见 §21。
验收明细见 `docs/plans/2026-10-20-installed-0.0.61-acceptance.md`。
Issue：仓库 Issues 已关闭（2026-10-19 实测 `has_issues=false`，创建接口返回 410），
本文件即项目记录；待仓库启用 Issues 后补编号与链接。
关联规格：

- `docs/specs/tauri-renderer-host.md`（Host 契约、单端口、启动/销毁顺序）
- `docs/specs/tauri-platform-commands.md`（M1 平台面缺口、`window.mode` 分类）
- `docs/specs/in-app-remote-desktop.md`（远程桌面唯一权威）
- `docs/specs/lan-discovery.md`、`lan-paired-devices.md`、`lan-host-server-mode.md`、`lan-remote-attach.md`、`lan-remote-computer-use.md`
- `docs/specs/remote-workspace-identity.md`、`remote-workspace-conversation-mirror.md`、`remote-workspace-bidirectional-attachments.md`、`remote-workspace-rpc-bulk-chunking.md`

## 1. 为什么单独立项

这不是一个 UI 修复，而是三块能力的**产品壳接线**缺失：

1. **LAN 管理面**：Tauri 产品 UI 调用的 `getLanAccessState`、`setLanAccessEnabled`、
   `createLanAccessPairCode`、`discoverLanPeers`、`pairLanPeer`、`getLanPairedPeers` 等
   在 `window.mode` 注入层仍返回拒绝 Promise；`removeLanPairedPeer`、`renameLanPairedPeer`
   是静默空操作。真机表现是「远程控制」页读取已配对设备失败。
2. **远程桌面**：`rdGetPeerCapability`、`rdStartSession`、`rdSendSignal`、`rdEndSession`、
   `onRemoteDesktopSessionEvent` 等同样是拒绝/空操作；设备页第一步就失败，会话事件也不可能到达。
3. **远程工作区**：`connectRemote` 直接拒绝。`.plan/browser-import-e2e-4.log:4395` 的实机记录是
   `lastConnectionStatus: "failed"`、`lastConnectionError: "not_implemented_until_m3: connectRemote"`。
   即使补上 `connectRemote`，renderer 还需要一个 **scoped service attachment** 才能拿到远端
   文件/Git/对话服务；Tauri 桥目前只投递一个全局 `MessagePort`。

**服务层不是从零重写**：`packages/desktop/src/host/lanAccessServer.ts`、
`lanRemoteAttach.ts`、`remoteDesktopServer.ts`、`remoteDesktopMediaRelay.ts`、
`remoteDesktopPeerClient.ts`、`windowRemoteConnectionRegistry.ts` 以及
`packages/server/src/http.ts` 的端点已经实现，相关 38 条 LAN/远控测试与 156 条 Web/手机测试
在 Electron 线全绿。缺口在 **Host sidecar 没有启动这些能力、没有把它们暴露成 RPC 频道，
产品 renderer 也没有走 Host 服务**。

**与旧口径的更正**：`docs/plans/2026-10-18-m3-lan-remote-status.md:13` 写「仍缺的是两台机器/
真实 UI 的人工验收，不是代码缺口」。这句话对 Electron 服务层成立，对 **Tauri 产品壳不成立**；
本立项就是对这条口径的收口。该文已加更正说明。

## 2. 目标与非目标

### 目标（本立项必须交付）

- **G1 LAN 管理面可用**：设置页可读/开关 LAN、创建配对码、移除客户端、重置令牌；
  可发现/配对设备、列出已配对设备与其工作区、重命名/删除配对记录。
- **G2 远程桌面可用**：被控设置、逐设备授权、设备能力探测、主控会话、信令/媒体事件、
  剪贴板文本同步；至少两台 Windows 实机完成一次双向会话。
- **G3 远程工作区可用**：LAN/SSH/WSL/Docker 的 `connectRemote`、取消、上下文绑定、
  dispose、日志/关闭/重连事件，以及 remote scope 的 scoped services attachment；
  `D:\gas-hub-xp` 这类已配对 LAN 工作区可以重连并同步对话/任务/文件。
- **G4 边界不退化**：Host 仍是业务状态唯一所有者；Rust 壳只做传输、窗口、OS 能力；
  renderer 只经 `IPlatformService` / `IServiceAccessor` 访问；不出现 `window.mode` 与
  Host 服务双写；`desktop-continuous` 与 `web-remote-replayable` 语义保持区分。

### 非目标（本立项不做）

- 不在 Rust 侧重写 LAN/RD/remote 业务（`crates/lan`、`crates/remote`、`crates/server`
  仍按 DR-M3-1 / DR-WEB-1 留给后续运行时迁移）。
- 不做公网 rendezvous/中继/打洞；范围仍是 LAN/Tailscale 直连。
- 不重写手机 Web UI；手机远控继续复用 Host attachment。
- 不做远程桌面音频、全显示器铺开、文件传输、虚拟显示器、多会话 tab、打印机重定向
  （见 `in-app-remote-desktop.md` 的 P3/P4）。
- 不在本立项里统一 Electron 安装身份或多窗口模型；但新接口必须保留 window-scoped 语义，
  为 M4 多窗口留出边界。
- 不新增 TLS/mTLS 或配对限速；现有威胁模型是「可信局域网」，若要加固另开决策。

## 3. 现状与缺口（证据）

| 层            | 已有                                                                                                                                                                                 | 缺口                                                                                       |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------ |
| shared 契约   | `packages/shared/src/platform.ts:596-655, 1006-1106` 的 LAN/RD/connectRemote 方法；`lanAccess.ts` 的配对/令牌/状态 schema；`remote-desktop.ts` 协议 v2                               | `ServiceChannels` 没有 LAN/RD 频道；`PlatformMethodPayload` 漏了 4 个 paired-peer 方法     |
| Host 业务     | `lanAccessServer.ts:218-487`、`lanRemoteAttach.ts:50-276`、`remoteDesktopServer.ts`、`remoteDesktopMediaRelay.ts`、`remoteDesktopPeerClient.ts`、`windowRemoteConnectionRegistry.ts` | 这些只在 Electron `host/index.ts` 装配；`host-standalone` 只启动 `createLocalServices`     |
| Tauri sidecar | 单窗口、单 `MessagePort`、stdio 帧泵；`host-standalone/index.ts:289-303` 先注册频道再 `ready()`                                                                                      | 没有 LAN 监听、没有 RD server、没有 remote registry；没有 scoped attachment 通道           |
| 产品 renderer | `packages/desktop/src/renderer/src/desktopPlatform.ts:35-72` 把 LAN/RD/connectRemote 原样转给 `window.mode`                                                                          | 没有 Host 服务委托；`window.mode` 的 LAN/RD 成员是 reject/no-op                            |
| Tauri 注入    | `crates/sidecar-host/src/window_mode.ts:146-178` / `:196-222` 的分类有静态测试守卫                                                                                                   | 分类是「未实现」的事实登记，不是接线；`tauri-invoke-shim` 只是 M1 验收台，不是产品 UI 路径 |
| OS 能力       | Electron main 有 `clipboard`、`desktopCapturer`、`screen`、consent/indicator 两个 `BrowserWindow`                                                                                    | Rust 壳没有等价命令/窗口；Host 侧不能直接枚举显示器或读系统剪贴板                          |

**结构性缺口有且只有两个**：

1. **Host 服务面**：把既有 LAN/RD/remote 装配从 Electron `host/index.ts` 抽成
   Electron-free 模块，在 `host-standalone` 里启动，并注册成 RPC 频道。
2. **Attachment 传输**：`connectRemote` 成功后，每个 remote logical session 需要独立的
   service scope；Electron 用 `MessageChannelMain` 每条 session 建一个 port，Tauri 桥只有
   一个 `MessagePort`（`crates/sidecar-host/src/bridge.ts:68-76, 147-161`）。必须新增
   attachment broker（多端口或单端口多路复用），否则「连上了但不同步」仍会发生。

### 关键调用链（迁移前）

```
renderer
  SettingsPage / RemoteDesktopHome / useRemoteWorkspaceHistory
    → desktopPlatform (window.mode)
      → Tauri 注入层: LAN/RD reject 或 no-op; connectRemote reject
Host sidecar
  createLocalServices only（file/git/mode-agent/...）
Rust shell
  window controls + host stdio pump + deep link
```

### 目标调用链（迁移后）

```
renderer
  IPlatformService（desktopPlatform）
    ├─ LAN / RD ──→ IServiceAccessor.lanAccessService / remoteDesktopService
    │                  └─ Host RPC over the existing MessagePort
    └─ 壳能力 ────→ window.mode → Rust（窗口/剪贴板/显示器/同意窗/退出）
Host sidecar
  ├─ lan-access（local-only）→ LanAccessServer + discovery + credentials
  ├─ remote-desktop → RD server/controller + media relay
  └─ remote registry → SSH/WSL/Docker/LAN transports + scoped attachments
Rust shell
  transport / windows / OS primitives / lifecycle
```

## 4. 架构决策

### D1：Host 拥有业务状态，产品 UI 走 `IServiceAccessor`

LAN/RD 的状态所有者保持现状：

- LAN 被控状态：`LanAccessServerHandle`（`lanAccessServer.ts:63-71`）；
- LAN 开关/逐设备授权：settings/credentials 服务；
- RD 被控状态：`RemoteDesktopHostServer`；主控会话状态：Host 的 controller/coordinator；
- remote workspace：`windowRemoteConnectionRegistry`（每窗口唯一）。

`window.mode` 不再新增业务写入路径。产品 `desktopPlatform.ts` 的 LAN/RD 方法改为
「有 Host 服务则委托，否则回退 `window.mode`」；每个方法切换时同步补测试。

### D2：管理频道 local-only，绝不暴露给 LAN 对端

`createHttpServer(services, ...)` 会把传入的 `ServiceCollection` 暴露给每个 LAN WS
（`packages/server/src/http.ts:165-221, 551-555`）。因此：

- `lan-access` 管理频道放进独立的 local-only `ServiceCollection`，只注册到窗口 renderer 的
  `ChannelServer`；
- 若未来需要对端调用的能力（capability/peer session），单独放进 peer-facing 集合，并在
  `serverRemoteInfoSchema.capabilities` 声明；
- 禁止把 `pairPeer`（会返回 token）暴露给对端。

### D3：不双写，`window.mode` 保留 fallback

迁移期间不会同时让 `window.mode` 和 Host 服务写同一状态。每个方法只有在 Host 服务、平台
委托、测试都就绪后才从 `REJECTING_REQUIRED`/`NO_OP_REQUIRED` 的语义中退出；在此之前保持
「明确失败」，不返回假成功。`removeLanPairedPeer` / `renameLanPairedPeer` 的静默 no-op 必须
优先移除——它们会让用户误以为操作成功。

### D4：`window-scoped Local Host` 与 remote registry 的边界不变

当前 Tauri 单窗口单 sidecar，等价于一个 window-scoped Host。将来 M4 多窗口时，每个窗口
各自拥有 Local Host 与 remote registry；不得把 `windowRemoteConnectionRegistry` 上收成 Rust
壳的全局连接表。

### D5：事件类型按幂等性区分

- 幂等状态快照：`onDidChange: Event<T>`（可缓冲，首个订阅者拿到最新状态即可）；
- 非幂等信令/增量：`onDynamicSessionEvent(...)`（不缓冲、不重放旧信令）；
- 事件订阅跨 `MessagePort` 复用现有 `ChannelServer` 机制，不新增 Rust 事件通道。

### D6：超时与并发语义统一

现状 main 侧 8s、Host 内部 pair/server-info 10s，可能出现「UI 已报超时、Host 却配对成功」。
本立项要求：

- 所有 UI 可见请求统一一个预算（建议 10s，或把 Host 内部压到 8s 内）；
- LAN 启动改为**单一启动 Promise + 显式状态机**，消除 `ensureLanAccessStarted` 的
  check-then-act 并发启动；
- sidecar 只有一个 LAN owner；不再需要 Electron 的 `pickOwner()` 多 Host 投票。

### D7：OS 能力留在壳，Host 只消费快照

Host 不能读系统剪贴板、枚举显示器或弹系统同意窗。建议拆分：

| 能力                      | 所有者                                                      |
| ------------------------- | ----------------------------------------------------------- |
| 显示器枚举/几何、光标位置 | Rust 壳（Tauri command）→ 需要时由 renderer 传给 Host       |
| 系统剪贴板读取            | Rust 壳（Tauri command）                                    |
| 同意窗/指示条             | Rust 壳的 `WebviewWindow`（推荐）或 renderer 模态（待决策） |
| 采集/编码/输入注入        | 既有 CUA helper（Host 侧）                                  |
| 媒体密钥/盐               | 被控 main/helper 与主控 renderer 内存（不在 Host 落盘）     |

## 5. 接口契约（第一批）

### 5.1 频道

在 `packages/shared/src/channels.ts` 增加：

```ts
ServiceChannels.LanAccess = "lan-access";
ServiceChannels.RemoteDesktop = "remote-desktop";
```

接口 descriptor 放 `packages/services/src`（browser-safe），实现放
`packages/desktop/src/host/`（Node-only），注册放
`packages/desktop/src/host-standalone/serviceAccessor.ts`。

### 5.2 `ILanAccessService`

```ts
interface ILanAccessService {
  getState(): Promise<LanAccessState>;
  setEnabled(enabled: boolean): Promise<LanAccessState>;
  createPairCode(): Promise<LanAccessState>;
  removeClient(clientId: string): Promise<LanAccessState>;
  resetTokens(): Promise<LanAccessState>;
  discoverPeers(): Promise<LanDiscoveredPeer[]>;
  pairPeer(request: LanPairPeerRequest): Promise<LanAccessPairResult>;
  listPairedPeers(): Promise<LanPairedPeer[]>;
  getPeerWorkspaces(serverId: string): Promise<LanPairedPeerWorkspaces>;
  removePairedPeer(serverId: string): Promise<void>;
  renamePairedPeer(serverId: string, name: string): Promise<void>;
  onDidChange: Event<LanAccessState>;
}
```

- 所有请求参数用对象，避免位置参数演进困难。
- `pairPeer` 返回的 token 只给本机 renderer；不得进入 peer-facing 集合。
- `removePairedPeer` / `renamePairedPeer` 必须真实落凭据，不能 no-op。

### 5.3 `IRemoteDesktopService`（第二阶段）

```ts
interface IRemoteDesktopService {
  getServerState(): Promise<RemoteDesktopServerState>;
  setServerEnabled(enabled: boolean): Promise<RemoteDesktopServerState>;
  authorizeClient(clientId: string): Promise<RemoteDesktopServerState>;
  revokeClient(clientId: string): Promise<RemoteDesktopServerState>;
  getPeerCapability(serverId: string): Promise<PeerCapability>;
  getControlledState(): Promise<{ controlled: boolean }>;
  startSession(
    serverId: string,
    quality?: RemoteDesktopQuality,
    codec?: RemoteDesktopCodec,
  ): Promise<{ ok: true; mediaUrl?: string } | { error: RemoteDesktopErrorCode }>;
  sendSignal(message: RemoteDesktopSignalMessage): Promise<void>;
  endSession(reason?: RemoteDesktopEndReason): Promise<void>;
  onDynamicSessionEvent(sessionId?: string): Event<RemoteDesktopSessionEvent>;
  onConsentRequest: Event<RemoteDesktopConsentRequest>;
  respondConsent(requestId: string, decision: "allow" | "deny", remember: boolean): Promise<void>;
}
```

`rdReadClipboardText` 留在壳能力面（Rust command），不经 Host。

### 5.4 远程工作区（第三阶段）

先在 `host-standalone` 启动 `windowRemoteConnectionRegistry`，再暴露：

```ts
remote.connect(request) → descriptor
remote.cancelConnect(requestId)
remote.bindContext(sessionId, workspacePath, workspaceIdentity)
remote.dispose(sessionId)
remote.attach(sessionId, scope) → scoped attachment
remote.detach(attachmentId)
remote.getSession(sessionId) / listSessions()
```

事件：`remote.connectionLog`、`remote.connected`、`remote.connectFailed`、
`remote.closed`、`botRemote.reconnected`。

**Attachment 传输是第三阶段的门槛**：在单端口多路复用与页面侧多端口 broker 之间做一次
spike（见 DR-LR-2），未通过前 `connectRemote` 保持明确失败，不能假装成功。

## 6. 工作分解

### LR-0 规格与契约（0.5–1 天）

- [x] 更新 `tauri-renderer-host.md`：新增 LAN/RD 频道、local-only 集合、attachment 契约。
- [x] 更新 `tauri-platform-commands.md`：登记本立项；明确 `window.mode` 与平台委托的关系。
- [x] 新增频道 descriptor 与类型；补静态契约测试（频道名、命令名、local-only 断言）。
- [x] 建立假 credentials/settings/startServer 的测试夹具。

验收：`pnpm typecheck`、静态契约测试红/绿各一次；没有产品行为变化。

### LR-1 LAN 管理面（2–3 天）

- [x] Host adapter：复用 `lanAccessServer`、`lanRemoteAttach`、`desktopLanDiscovery`；
      显式 dispose；动态 import 以绕开 stdout 守卫。
- [x] 注册 local-only `lan-access` 频道到 `host-standalone`；启动时按
      `lanAccessEnabled` 恢复监听，失败只告警并回 disabled。
- [x] `IServiceAccessor.lanAccessService` + `RemoteServiceAccess` 代理。
- [x] `desktopPlatform.ts` 的 11 个 LAN 方法委托 Host 服务，保留 `window.mode` fallback。
- [x] 修复 `removeLanPairedPeer` / `renameLanPairedPeer` 的静默 no-op。
- [x] 测试：disabled state、enable/disable、pair code 一次性、paired peers 排序、
      workspaces、rename/remove、local-only 不被 peer 集合看到、事件订阅。
- [ ] 真机：设置页 LAN 卡片、发现、配对、已配对设备管理、工作区目录读取。

验收：Tauri 产品 UI 不再出现「读取已配对设备失败」；LAN 开关与配对闭环可重放。

### LR-2 远程桌面设置面（1–2 天）

- [x] Host adapter：`remoteDesktopServer` 的 server state / enable / authorize / revoke。
- [x] `remote-desktop` 频道；设置页与 LAN client 记录仍由同一所有者写。
- [x] `rdGetPeerCapability` / `rdGetControlledState` 接 Host 或壳能力。
- [x] 测试：server state、授权三态、peer capability、关闭门控。

验收：设备页能列出已配对设备并显示能力；被控开关与逐设备授权可用。

### LR-3 远程桌面会话（1–2 周）

- [x] 把 controller/coordinator 从 Electron main 抽到 Host（保留状态机与单会话互斥）。
      **主控 controller 与被控协调器均已落地**（`host/remoteDesktopController.ts`、
      `host/remoteDesktopControlledCoordinator.ts`）；Electron 线保持原实现。
- [x] 信令/媒体事件经 `onDynamicSessionEvent` 到 renderer；不缓冲旧信令。
- [x] 壳能力：`rdReadClipboardText`、显示器枚举/几何、光标位置。
      **`rdReadClipboardText` 已是壳命令（Windows CF_UNICODETEXT）；显示器枚举/几何由
      renderer 经 Tauri `availableMonitors()` 推给 Host；光标位置未用到（随帧光标）。**
- [ ] 同意窗/指示条：推荐 Rust `WebviewWindow`；60s 超时、拒绝、结束按钮。
      **renderer 同意模态与指示条均已落地（60s、允许/拒绝/记住、结束会话）；壳窗口仍待产品决策。**
- [x] 窗口关闭 / 应用退出：先发 bye/end-session，再收 Host/sidecar
      （Host 被控 server `dispose` 先发 `bye{remote-end}`；壳侧退出顺序仍待真机验收）。
- [x] 测试：controller WS、coordinator 状态机、media relay、clipboard gate、退出收口。
      **controller WS / 被控协调器 / 媒体中继 / 剪贴板门 / 退出均已覆盖；真机多屏与重连待验。**
- [ ] 两台 Windows 实机验收（主控/被控、授权、多屏切换、剪贴板、断连重连）。

验收：`in-app-remote-desktop.md` 的实机清单通过；旧对端仍按能力位降级。

### LR-4 远程工作区 attachment（1–2 周）

- [x] Spike：单端口多路复用 vs 页面多端口 broker；给出跨语言改动量、失败模式、测试方案。
      结论与可执行验证见 `docs/plans/2026-10-19-lr4-attachment-spike.md`（推荐单端口多路复用，
      Rust 桥不改，base 帧格式不变）。
- [x] 选定方案后实现 attachment broker：local + N 个 remote scope 的路由与回收。
      **已完成**：`frameMultiplexer` + Host 解复用 + 页面桥 scoped port +
      `scope-ready/scope-detach` 控制帧 + `remote` 频道的 attach/detach 与 scope 回收。
- [x] `windowRemoteConnectionRegistry` 进 standalone Host；`connectRemote`/cancel/bind/dispose。
      **LAN 已接**；SSH/WSL/Docker 在 Tauri Host 明确返回不支持（不伪装成功）。
- [x] scoped services attachment 到 renderer；`remoteSessionId + workspacePath + workspaceIdentity`
      三元组严格校验。
      **已接**：`remote.attach` 用 registry 的 scoped scope 校验三元组，scoped
      `ServiceCollection` 经 `ChannelServer` 暴露；renderer 用既有 `ScopedServicePort` 消息消费。
- [ ] 事件：connection log、closed、bot reconnect；UI tab 降级与重连。
      connection log / closed 已接；bot reconnect 于 2026-10-22 接通（见 §22）；
      UI tab 断连降级逻辑已实现（`onRemoteSessionClosed` → 清 `remoteSessionId` +
      running 任务收口）并于同日补纯逻辑回归（见 §25）；自动重连按**决策 B**落地：
      1s/3s/9s 最多 3 次、干净退出与鉴权/版本错误不重试（见
      `docs/specs/remote-workspace-auto-reconnect.md`）。启动仍不自动连接。
- [x] 测试：connect/cancel/bind/dispose、attachment 路由、Host 退出、stale session、
      `REMOTE_WORKSPACE_IDENTITY_MISMATCH`、断线重连。
      connect/bind/attach/detach/dispose、attachment 路由、Host 退出、stale session 已覆盖；
      三元组不匹配由 registry 的 scope 校验覆盖（`attach` 不接受外部 scope，无法构造不匹配）；
      断线重连的 UI E2E 仍待真机。

验收：LAN 已配对工作区可重连；对话/任务/文件/Git 走 remote scope；关闭 tab 释放 session。

### LR-5 验收与发布（2–3 天）

- [ ] 两台 Windows 实机：LAN 发现→配对→attach→prompt→流式结果。
- [ ] 远程桌面双向授权、剪贴板、多屏、断连重连。
- [ ] 手机/Web 远控复用 Host attachment 的回归（`web-remote-replayable`）。
- [ ] Tauri 产品 UI E2E：设置页、设备页、连接向导、工作区重连。
- [ ] 逐项移除 `window.mode` 的 rejecting/no-op 条目，并更新注入测试的期望集合。
      〔2026-10-22：`createTempTextAttachment` 与 `openExternalFile` 已从
      `REJECTING_REQUIRED` 升级为壳命令（`crates/platform/src/commands/temp_attachment.rs`、
      `shell.rs`），注入测试与验收台已同步；其余条目仍待逐项迁移。〕
- [ ] 发布说明写清支持矩阵；Windows-only 的 RD 不对外宣称跨平台。

验收：LR-1..LR-4 的测试与真机项全绿；没有双写路径；Host 崩溃/退出不留孤儿会话。

## 7. 测试与门禁

每个阶段必须跑：

```bash
pnpm typecheck
pnpm lint
pnpm --filter @mode/ui test
pnpm --filter @mode/desktop-tauri test
pnpm architecture:check --changed
node scripts/check-language-policy.mjs
```

新增测试层次：

1. **静态契约**：频道名/命令名/local-only 集合；`window.mode` 分类与 renderer 直接调用扫描。
2. **端口往返**：spawn 生产 Host，真 `MessagePort` + `ChannelClient`，调 LAN state、
   订阅事件、断言销毁顺序。
3. **适配器单测**：假 credentials/settings/startServer，覆盖 disabled、enable、pair code、
   rename/remove、onDidChange、dispose。
4. **平台委托**：有 Host 服务时不调 `window.mode`；无服务时 fallback；事件 disposer 包装。
5. **真机**：两台 Windows + 真实 LAN；手机/Web 回归。

禁止用超时掩盖同步问题：每个失败路径要有明确错误码与可观察日志。

## 8. 风险与开放决策

| ID      | 决策/风险                                            | 建议                                                                                                                                                                    |
| ------- | ---------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| DR-LR-1 | 远程桌面同意窗/指示条放壳还是 renderer               | 推荐壳 `WebviewWindow`：能在窗口最小化/焦点外显示，权限边界更清晰；renderer 模态作为备选。需产品确认视觉与交互。                                                        |
| DR-LR-2 | attachment 传输：单端口多路复用 vs 页面多端口 broker | 推荐先 spike 单端口多路复用：Rust 桥不动，只改 `@mode/rpc` 与两端路由；代价是 attachment id 必须贯穿所有帧。spike 以「两个 remote session 并发 + 关闭回收」为通过条件。 |
| DR-LR-3 | 显示器/剪贴板/光标由谁读                             | 推荐壳读快照，Host 消费；避免 Host 依赖 Tauri/平台 API。                                                                                                                |
| DR-LR-4 | LAN 安全加固（TLS、配对限速）                        | 本立项不做；在发布说明标注「可信局域网」。若要做，另开安全决策，两端一起改。                                                                                            |
| DR-LR-5 | 与 Electron 线并存/回滚                              | Electron 线保持现状；Tauri 迁移不得改 Electron main 的 LAN/RD 写入路径。若将来共享 Host 服务，必须先把 Electron IPC 迁成服务客户端，避免双写。                          |
| 风险    | 端口/监听冲突                                        | sidecar 单 owner；固定 45880/45879 与旧 Electron 兼容，但需处理 EADDRINUSE（当前实现无回退）。                                                                          |
| 风险    | token 进入 renderer                                  | 现状 `pairLanPeer` 返回 token 给 renderer；迁移可维持，但需在文档修正「令牌不出 host」的错误注释，或改成只持 `serverId`。                                               |
| 风险    | schema 漂移导致 8s 超时                              | 复用 strict schema；新增字段必须同步 `validation.ts`；回帖字段有历史事故。                                                                                              |
| 风险    | Host 崩溃/退出                                       | 现状无自动重启；远程 session 必须 fail fast 并让 UI 降级，不能继续路由到失效 attachment。                                                                               |
| 风险    | 多窗口                                               | 当前单窗口；M4 多窗口时每个窗口独立 Host/registry，不能上收 Rust。                                                                                                      |

## 9. 交付方式

- 每个阶段一个 PR，PR 正文关联本 Issue；不把 LR-1..LR-4 塞进一个巨型 PR。
- 分支建议：`feat/lr1-lan-service`、`feat/lr2-rd-settings`、`feat/lr3-rd-session`、
  `feat/lr4-remote-attach`。
- LR-1 可在 v0.0.60 之后独立发布；LR-3/LR-4 在真机验收通过前不对外宣称可用。
- 回滚：`window.mode` 的 rejecting/no-op 是天然 fallback；每个阶段可单独 revert，
  不触碰 Electron 线。
- 发布说明：Tauri 版 LAN/RD/远程工作区的支持矩阵；远程桌面仅 Windows；macOS/Linux
  是否支持 RD 需单独确认。

## 10. 第一刀（建议立即执行）

1. LR-0：写 `lan-access` 的 descriptor/类型/静态契约测试，确定 local-only 注册方式。
2. LR-1：Host adapter + `host-standalone` 注册 + `desktopPlatform` 委托 + 真机设置页验证。
3. 在 LR-1 合并后再决定 LR-2/LR-3 的顺序；LR-4 的 attachment spike 可与 LR-1 并行。

**完成定义**：Tauri 产品 UI 在无 Electron main/preload 的情况下，能完成 LAN 配对与设备管理；
远程桌面和远程工作区按阶段验收；所有状态写入仍只有 Host 一个所有者，`window.mode` 不再
承担业务实现。

## 11. 执行记录（2026-10-19，LR-0 + LR-1）

落点：

- 频道与契约：`ServiceChannels.LanAccess` / `RemoteDesktop`；`packages/services/src/lan-access/lanAccessService.ts`；
  `IServiceAccessor.lanAccessService` 与 `RemoteServiceAccess` 代理。
- Host 实现：`packages/desktop/src/host-standalone/lanAccessService.ts`（单启动 Promise、显式 dispose、
  恢复失败只告警）；注册在 `host-standalone/serviceAccessor.ts` 的独立 local-only 集合。
- 共用逻辑：`host/lanPairedPeers.ts`（Electron Host 与 Tauri Host 的唯一实现）、
  `host/lanWorkspaceList.ts`、`host/lanDiscovery.ts`（从 `main/` 移到 Electron-free 的 `host/`）。
- 平台委托：`desktopPlatform.ts` 的 11 个 LAN 方法；`main.tsx` 用 getter 注入 Host 服务。
  `removeLanPairedPeer` / `renameLanPairedPeer` 在 Tauri 无 Host 服务时**显式拒绝**，不再回落成 no-op。

自动化验证（本机 2026-10-19）：

- `pnpm typecheck` 通过；
- `pnpm --filter @mode/services test`：383 项，375 pass / 8 skip / 0 fail；
- `pnpm exec tsx --test packages/desktop/tests/*.test.mts`：123 项全绿（含新增 4 个 LR-1 用例）；
- `pnpm --filter @mode/desktop-tauri test`：44 项，37 pass / 7 skip / 0 fail；
- `pnpm architecture:check --changed`：0 violations；
- `node scripts/check-language-policy.mjs`：通过；
- `pnpm lint` 仅剩 `.plan/` 下未跟踪的浏览器 profile 产物报错（2 条 max-lines，与本次改动无关）；
  对本次改动的 12 个源文件单独跑 oxlint：0 error。

未完成：两台 Windows 真机的 LAN 配对/设备管理验收（LR-1 最后一条），以及 LR-2..LR-5。

## 12. 执行记录（2026-10-19，LR-2）

落点：

- 契约：`packages/services/src/remote-desktop/remoteDesktopService.ts`（`IRemoteDesktopService`）；
  `@mode/shared/remote-desktop` 新增 `RemoteDesktopServerState` / `RemoteDesktopPeerCapability`，
  Electron main 的 settings.ts 改为复用同一类型。
- Host 实现：`packages/desktop/src/host-standalone/remoteDesktopService.ts`。它不直接写凭据，
  而是把 RPC 调用翻译成 `RemoteDesktopHostServer` 既有消息命令，经 `postToMain` 按 `requestId`
  收状态快照；`getControlledState` 读 server 的 busy 事实（同意等待/待接线/进行中）。
- 装配：`host-standalone/serviceAccessor.ts` 先建 RD server、再建 LAN adapter
  （LAN 握手使用 RD 能力位与 hooks），两者都注册在 local-only 集合；dispose 顺序
  LAN → RD → 服务资源。`createLocalServices` 补上 `isRemoteDesktopServerEnabled` 闭包，
  保证被控开关打开时 CUA Helper 的创建准入与 Electron 线一致。
- 平台委托：`desktopPlatform.ts` 的 6 个被控设置面方法；主控会话（start/signal/end/
  clipboard/session events）仍走壳，未被提前接管。
- `RemoteDesktopHostServer` 新增只读 `isControlled()`（additive，不改 Electron 路径）。

自动化验证（本机 2026-10-19）：

- `pnpm typecheck` 通过；
- `pnpm --filter @mode/services test`：383 项，375 pass / 8 skip / 0 fail；
- `pnpm exec tsx --test packages/desktop/tests/*.test.mts`：134 项全绿
  （新增 `remote-desktop-host-service`、`remote-desktop-port`，并扩充平台委托与静态契约用例）；
- `pnpm --filter @mode/desktop-tauri test`：44 项，37 pass / 7 skip / 0 fail；
- 本次改动的 16 个文件单独跑 oxlint：0 warning / 0 error。

未完成：LR-2 的 Windows 真机设备页验收（已配对设备、能力位、开关与逐设备授权），以及 LR-3..LR-5。

## 13. 执行记录（2026-10-19，LR-3 主控路径）

落点：

- 主控 controller：`packages/desktop/src/host/remoteDesktopController.ts`（Electron-free，
  状态机与 `main/remoteDesktop/controllerSession.ts` 同义：单会话互斥、connecting/connected/
  closed、bye/error 收口、传输断开 retryable、dispose 带出 bye）。Electron 线未改。
- 频道契约：`IRemoteDesktopService` 增加 `startSession` / `sendSignal` / `endSession` /
  `respondConsent` / `onDynamicSessionEvent` / `onConsentRequest`；
  `@mode/shared/remote-desktop` 增加 `RemoteDesktopSessionEvent`、`RemoteDesktopConsentRequest`、
  `RemoteDesktopStartSessionResult`。
- 同意流：Host 收到被控 server 的 `ConsentRequest` 后，已记住设备直通；否则发
  `onConsentRequest` 并等 60s（可注入），超时按 `consent_timeout` 拒绝；首个订阅者会补上
  仍在等待的请求；Host 退出时拒绝挂起请求。
- 平台委托：`desktopPlatform.ts` 的 `rdStartSession` / `rdSendSignal` / `rdEndSession` /
  `onRemoteDesktopSessionEvent` 在 Tauri 下走 Host；`rdReadClipboardText` 仍走壳。
- 被控侧 coordinator（monitors/media/clipboard/指示条）未迁：Tauri 被控端还不能完成会话；
  这是 LR-3 的剩余主体，依赖壳能力（显示器枚举、剪贴板、同意窗）。

自动化验证（本机 2026-10-19）：

- `pnpm typecheck` 通过；
- `pnpm --filter @mode/services test`：383 项，375 pass / 8 skip / 0 fail；
- `pnpm exec tsx --test packages/desktop/tests/*.test.mts`：146 项全绿
  （新增 controller 4 项、同意流 7 项、端口往返主控路径、平台委托）；
- `pnpm architecture:check --changed`：0 violations；语言政策检查通过；
  本次改动文件 oxlint 0 warning / 0 error；
- `pnpm --filter @mode/desktop-tauri test`：**36 pass / 1 fail / 7 skip**。失败项是
  `windowModeInjection.test.mts` 的「可选成员 onUpdateStateChanged 不得被注入」，
  来自工作区里另一条未提交的 update-state 改动（`crates/sidecar-host/src/window_mode.ts`
  15:45 被外部修改），与 LR-3 改动无关；LR-2 收口时该套件为 37 pass / 0 fail。

未完成：被控侧 coordinator、壳能力（剪贴板/显示器/光标）、同意窗/指示条、两台 Windows 实机验收。

## 14. 执行记录（2026-10-19，LR-4 spike）

- 文档：`docs/plans/2026-10-19-lr4-attachment-spike.md`（协议、生命周期、失败模式、生产化步骤）。
- 可执行验证：`packages/desktop/tests/attachment-multiplexer-spike.test.mts`。
  同一根传输并发 base + 两个 remote scope、256KB 大帧、scope 回收、未知 scope 丢弃，
  2 项全绿。原型内联在测试里，**没有改生产传输层**。
- 结论：选单端口多路复用；帧格式为 `0xFF + scopeId(u32 BE) + 原帧`，base 帧逐字节不变，
  Rust 桥零改动。页面多端口 broker 被否（壳到 Host 只有一条 stdio 管道，仍要在 Rust 里做
  同样的解复用，改动更大且没有收益）。
- 下一步：按文档 §7 生产化（`frameMultiplexer` → Host 解复用 → 页面桥 scoped port →
  renderer 注册 → `remote.attach/detach` → 端口往返用例）。

## 15. 执行记录（2026-10-19，LR-4 传输层生产化）

落点：

- `packages/rpc/src/frameMultiplexer.ts`：scoped 帧编解码、demux、`scope-ready/scope-detach`
  控制帧；base 帧逐字节不变。
- `packages/desktop/src/host-standalone/index.ts`：stdin 先处理控制帧、再解 RPC scope；
  base 仍走原 `ChannelServer`；scoped server 按 id 注册，收到 `scope-ready` 才 `ready()`。
  测试入口 `MODE_HOST_TEST_SCOPES=1`（`attachTestScope`）。
- `crates/sidecar-host/src/bridge.ts`：`createScopedServicePort` / `releaseScopedServicePort`；
  入方向按 marker 分流；renderer 的 `ScopedServicePortReady` 翻译成 `scope-ready` 控制帧。
  Rust 壳未改。
- `apps/desktop-tauri/test/fixtures/productionHost.mts`：页面侧测试夹具也做解复用，
  新增 `createScopedClient` / `sendControl`。

自动化验证（本机 2026-10-19）：

- `pnpm typecheck` 通过；
- `pnpm exec tsx --test packages/desktop/tests/*.test.mts`：**153 项全绿**
  （新增 frame multiplexer 4 项、生产 Host scoped 端口往返 1 项、spike 2 项）；
- `pnpm --filter @mode/desktop-tauri test`：**37 pass / 1 fail / 7 skip**。失败项仍是工作区里
  另一条未提交 update-state 改动引起的 `onUpdateStateChanged` 可选成员断言，与 LR-4 无关；
  新增的 bridge scoped port 用例通过。
- 本次改动文件 oxlint 0 warning / 0 error。

未完成：Host `remote.attach/detach` 与 `windowRemoteConnectionRegistry` 进 standalone、
scoped services 的 registry 绑定、三元组校验、远程工作区事件与 UI 降级。

## 16. 执行记录（2026-10-19，LR-4 LAN 远程工作区）

落点：

- `IRemoteService`（`packages/services/src/remote/remoteService.ts`）：connect / cancel /
  bindContext / dispose / attach / detach / getSession / listSessions /
  `onDynamicConnectionEvent`。
- `packages/desktop/src/host-standalone/scopedAttachments.ts`：scope 登记表从入口抽出，
  统一 attach/start/detach/dispose 与 `onDidDetach`。
- `packages/desktop/src/host-standalone/internalServiceMembers.ts`：适配器的 Host 内部生命周期
  方法（`dispose`/`getActiveServer`/`getLanHooks`/`handleMessage`/`disposeHost`）改为不可枚举，
  防止 `ProxyChannel.fromService` 把它们暴露成 renderer 可调的 RPC command。
- `packages/desktop/src/host-standalone/remoteWorkspaceService.ts`：
  - `windowRemoteConnectionRegistry` 进 standalone Host；
  - LAN 连接复用 `attachLanRemoteConnection` + `createRemoteWorkspaceServiceCollection`；
  - `attach` 用 registry 的 remote scope（remoteSessionId + workspacePath + workspaceIdentity）
    取 scoped `ServiceCollection`，经 guarded `ChannelServer` 暴露；
  - SSH/WSL/Docker 明确返回「暂不支持」；
  - connection log / connected / connectFailed / closed 事件。
- `packages/desktop/src/renderer/src/desktopRemotePlatformBridge.ts`：
  - `connectRemote` → Host `remote.connect` → `remote.attach` →
    页面桥 `createScopedServicePort`；
  - `bindRemoteWorkspaceSessionContext` 换代 attachment（新 scope 建好后释放旧 scope）；
  - `disposeRemoteSession` / `cancelPendingRemoteConnection` / log/closed 事件委托；
  - 全部保持 `window.mode` 回退（Electron 线不变）。
- 测试：`remote-workspace-host-service.test.mts`（假 LAN transport + 真 scoped 端口）、
  平台委托用例扩充（connect/bind/dispose/事件/Electron 回退）、静态契约（remote 频道 local-only）。

自动化验证（本机 2026-10-19）：

- `pnpm typecheck` 通过；
- `pnpm exec tsx --test packages/desktop/tests/*.test.mts`：**164 项全绿**；
- `pnpm --filter @mode/desktop-tauri test`：37 pass / 1 fail / 7 skip；失败项仍是工作区里
  另一条未提交 update-state 改动引起的 `onUpdateStateChanged` 可选成员断言，与 LR-4 无关；
- 本次改动文件 oxlint 0 warning / 0 error。

未完成：SSH/WSL/Docker、bot reconnect 事件、UI tab 降级与断线重连、
stale session / `REMOTE_WORKSPACE_IDENTITY_MISMATCH` 测试、真机 LAN 配对→attach→prompt 验收。

## 17. 执行记录（2026-10-19，LR-3 被控协调 + 同意窗）

落点：

- `packages/desktop/src/host/remoteDesktopControlledCoordinator.ts`：被控侧纯协调器
  （hello→monitors、select-monitor→media-open+MediaStart/MediaSwitch、
  MediaStatus→freeze/unfreeze、SessionClosed→MediaStop）；媒体密钥/盐只在 Host 内存。
- `remoteDesktopService`：`setLocalDisplays`（renderer 推送显示器快照）、协调器接线、
  `MediaStatus`/`SignalFromHost`/`SessionClosed` 路由；被控 server `dispose` 先发
  `bye{remote-end}`，controller `error` 在 server 层收口。
- `packages/desktop/src/renderer/src/desktopLocalDisplays.ts`：Tauri `availableMonitors()`
  → 物理矩形快照，启动与窗口 resize/move 去抖推 Host。
- `RemoteDesktopConsentDialog`（`packages/ui/src/remote-desktop/`）：Tauri 被控同意模态，
  60s 倒计时、允许/拒绝/记住；复用既有 i18n。`IPlatformService` 增加两个可选方法，
  Electron 不实现（继续用 main 原生窗）。
- 测试：被控协调器 6 项、显示器映射/推送 3 项、adapter 协调路由 1 项。

自动化验证（本机 2026-10-19）：

- `pnpm typecheck` 通过；
- `pnpm exec tsx --test packages/desktop/tests/*.test.mts`：**174 项全绿**；
- `pnpm --filter @mode/ui test`：408 项全绿；
- `pnpm --filter @mode/desktop-tauri test`：37 pass / 1 fail / 7 skip；失败项仍是工作区里
  另一条未提交 update-state 改动引起的 `onUpdateStateChanged` 断言，与本迁移无关；
- 本次改动文件 oxlint 0 warning / 0 error。

未完成：rd_media helper 打包形态、
两台 Windows 实机（授权/多屏/剪贴板/断连重连）。

## 18. 执行记录（2026-10-19，LR-3 被控指示条）

落点：

- `RemoteDesktopHostServer.onDidChangeControlledState`：同意等待 / 待接线 / 进行中会话的
  布尔状态变化事件（只在真的变化时触发）。
- `remoteDesktopService.onDidChangeControlledState`：幂等事件，首个订阅者补当前值；
  `IPlatformService.onRemoteDesktopControlledStateChanged` 只在 Tauri 暴露。
- `RemoteDesktopIndicator`（renderer 覆盖层）：显示「本机正在被远程控制」/「{device} 正在
  远程控制本机」，带「结束」按钮（走 `rdSetServerEnabled(false)` 同一写路径）。
- i18n：三套语言补 `remoteDesktop.indicator.controlling`（`indicator.text` / `indicator.end` 复用既有键）。

自动化验证（本机 2026-10-19）：

- `pnpm typecheck` 通过；
- `pnpm exec tsx --test packages/desktop/tests/*.test.mts`：**175 项全绿**；
- `pnpm --filter @mode/ui test`：408 项全绿；
- 本次改动文件 oxlint 0 warning / 0 error。

未完成：rd_media helper 打包形态、两台 Windows 实机验收。

## 19. 执行记录（2026-10-19，LR-3 剪贴板壳能力）

落点：

- `crates/platform/src/commands/clipboard.rs`：`rdReadClipboardText` 壳命令（Windows 读
  CF_UNICODETEXT：OpenClipboard 200ms 争用重试、GlobalSize 上界、无文本/被占用返回空串、
  从不抛；非 Windows 返回空串）。
- 命令切片 10 → 11：`command_slice!`、`crates/platform/tests/commands.rs` 的 M1_SLICE、
  `window_mode.ts` 的 M1_COMMANDS 与实现、`tauri-invoke-shim.ts` 的 M1_COMMANDS
  （同时从 M1_UNIMPLEMENTED_REQUIRED_METHODS 移除）。
- `packages/ui/test/tauriInvokeShim.test.ts` 的切片条数断言同步更新。
- `crates/platform/tests/clipboard.rs`：命令可调用且不 panic 的冒烟用例（不写剪贴板）。

自动化验证（本机 2026-10-19）：

- `node scripts/rust-cargo.ts test -p platform`：全部通过（含 clipboard 冒烟与 commands 守卫 4 项）；
- `pnpm --filter @mode/ui test`：408 项全绿；
- `pnpm --filter @mode/desktop-tauri test`：37 pass / 1 fail / 7 skip（失败项仍是工作区里
  另一条未提交 update-state 改动引起的 `onUpdateStateChanged` 断言，与本迁移无关）；
- `pnpm typecheck` 通过；本次改动文件 oxlint 0 warning / 0 error。

## 20. 执行记录（2026-10-19，预发布供真机验收）

为满足两台 Windows 真机验收，用 `workflow_dispatch` 发了一个预发布（不顶掉 Latest，
现有用户不会被自动更新拉进来）：

- tag：`v0.0.61-audit.20261019`，指向 `aac5c154`；
- run：`39278288517`，`release` / `verify` / fnos / macOS / Windows Tauri / Linux Tauri /
  Electron diagnostic / checksums / publish 全部 success；
- 产物：`Mode-0.0.61-audit.20261019-win-x64.exe`（51 MB）、macOS dmg、Linux AppImage、
  fnos fpk、`latest.json`、`sha256.txt`；
- 验收范围：LR-1 设备页（LAN 开关/配对/已配对设备与能力位）、LR-3 远程桌面（授权/同意弹窗/
  多屏/剪贴板/断连重连）、LR-4 LAN 远程工作区（attach、对话/任务/文件/Git、关闭 tab 释放）。
- 已知未完成：SSH/WSL/Docker 工作区返回不支持；bot reconnect 与 UI 断线重连降级未做；
  远程桌面仅 Windows；LAN 按可信局域网模型（无 TLS）。

验收通过后再发正式版 `v0.0.61`，并在发布说明写支持矩阵。

## 21. 远程桌面「对端不支持」根因（2026-10-20 本机复现与修复）

0.0.61 验收把远程桌面失败归为「对端缺 `mediaTicket`」，但 2026-10-20 用已配对令牌直接
请求对端（同一 `0.0.0-dev` 构建）得到 **HTTP 200 + ticket + mediaTicket**；随后本机
完整链路（会话 POST → 信令 WS → `hello` → `select-monitor`）在 `media-open` 后立刻收到
对端 `error{unsupported}`，对端日志值为：

```
rd_media-start-failed: Computer Use Helper is not reachable for rd_media
```

根因不在票据，而在 **Tauri 安装包没有带 Windows CUA helper 运行时**，且 Host sidecar 是
纯 Node 进程（没有 Electron 注入的 `process.resourcesPath`）：

1. `resolveWindowsCuaRuntime` 的打包分支要求 `process.resourcesPath`；纯 Node 下恒为
   `undefined` → `missing-resources-path`；
2. 即使补上基准目录，Tauri 的发运集也只有 `host-sidecar/node/config/agent`，没有
   `tools/cua-helper`。

两者任一成立都会让 `rd_media_start` 固定失败，被控端按既有失败语义回
`error{unsupported}`，主控 UI 于是显示「对端不支持远程桌面」。能力位探测、配对令牌、
会话 POST 与双票据全部正常——这正是「设备页可用、一进会话就说对端不支持」的原因。

修复：

- Host 入口在装配服务前把 `MODE_HOST_RESOURCES_DIR` 桥接为 `process.resourcesPath`
  （已存在不覆盖，相对路径不采用），见 `host-standalone/runtimeResources.ts`；
- `host-sidecar-ship` 把 `packages/desktop/bundled-tools/win32-x64/cua-helper` 三件套
  stage 进 `ship/tools/cua-helper`，Windows 平台配置把它映射到安装目录
  `tools/cua-helper`；
- `tauri` job 的 Windows 腿新增 `pnpm build:cua-helper`（msvc 目标），
  `verify-installed` 增加三件套在位检查。

复现口径（不依赖 UI）：用本机配对令牌对 `127.0.0.1:45880` 走
`POST /api/remote-desktop/session` → 信令 WS `hello` → `select-monitor`；修复前在
`media-open` 后收到 `unsupported`，修复后应进入媒体 WS 并持续收帧。

## 22. 执行记录（2026-10-22，LR-4 bot reconnect）

落点（Tauri Host 内直连，不再经 `parentPort → main`）：

- `packages/shared/src/remoteTarget.ts`：新增 `isSameRemoteTarget`——按 spec 的 authority
  口径覆盖 lan/ssh/wsl/docker；secret 与展示字段不参与。判等函数从已删除的
  `packages/desktop/src/main/remoteTargetMatch.ts` 收口到 shared，Host 与 services 共用。
- `packages/services/src/bots/botRemoteWorkspaceBridge.ts`：新增
  `BotRemoteWorkspaceTransport`（`queryConnectionStatus` / `reconnect` / `createRuntimePort`）。
  无 `parentPort` 但有直连传输时照常创建；查询失败返回 null 让桥回退本地标记；
  runtime port 的释放函数随桥 `dispose` 一起关闭。
- `packages/desktop/src/host-standalone/remoteWorkspaceService.ts`：
  - `queryBotConnectionStatus`：`isSameRemoteTarget` + `workspaceIdentity` 扫 online session；
  - `reconnectBotWorkspace`：同 target + identity 复用，否则 `registry.connect`，成功后
    `events.fire({ kind: "botReconnected" })`（target 脱敏）；
  - `createBotRuntimePort`：`MessageChannel` + `ChannelServer` 暴露 scoped 服务集合，
    端口直接交给 Bot 桥；不经页面桥/帧解复用，Host 退出统一关闭；
  - 三个方法都加入 `hideInternalServiceMembers`（`createBotRuntimePort` 返回 MessagePort，
    绝不能进 renderer RPC 面）。
- `packages/desktop/src/host-standalone/serviceAccessor.ts`：注入直连传输（延迟绑定到
  `remote` 服务）；`createLocalServices` 新增 `botRemoteWorkspaceTransport` 选项。
- `packages/desktop/src/renderer/src/desktopRemotePlatformBridge.ts`：
  `onBotRemoteWorkspaceReconnected` 订阅 Host 动态事件；`desktopPlatform` 删除直接委托
  `window.mode` 的那一行（Host 服务缺席时才回退）。

自动化验证（本机 2026-10-22）：

- `pnpm typecheck` 通过；
- `pnpm --filter @mode/shared test`：95 项全绿（新增 5 项 target 判等）；
- `packages/services/test/bot-remote-workspace-transport.test.ts`：5 项全绿；
- `packages/desktop/tests/remote-workspace-host-service.test.mts`：10 项全绿
  （新增 bot 状态/重连/进程内 runtime port RPC）；
- `packages/desktop/tests/desktop-platform-lan-delegation.test.mts`：10 项全绿
  （新增 `botReconnected` 事件委托与 disposer）。

未完成：UI tab 降级（远端断开后的 tab 状态与自动重连）仍待真机；SSH/WSL/Docker 真机未验。

## 23. 执行记录（2026-10-22，数据根迁移夹具演练）

落点：

- `packages/services/test/data-root-host-drill.test.ts` 新增「数据根迁移演练」：合成旧
  `.zcodium`（含 `v2/setting.json` 与一份 db 字节），用
  `initializeDataRootNonInteractive({ action: "migrate" })` 走真实复制（归属文件 + 旧根保留），
  再启动**生产 Host**，断言：
  - Host 的 `setting.get()` 读到旧根里的 `locale`（迁移结果真的被消费）；
  - `setting.update()` 的写入落在新根；
  - 旧根 setting 文件逐字节不变（只复制、不删除）。
- `packages/services/test/fixtures/hostStdioClient.ts`：演练用的最小生产 Host 客户端
  （真实 stdio 帧 + Initialize + 超时；不复制页面端口模型，跨包只按进程边界交互）。

**演练暴露的缺口（登记在 `docs/specs/mode-data-root.md`「Tauri Host 线的现状」）**：
Host sidecar 不做数据根初始化（归属文件、迁移都只有 server 入口在执行）；现有 Tauri 安装的
`.mode` 没有归属文件，直接接非交互 `fresh` 会被判成 `unowned` 并整体备份让路。接线前必须先决定
「无归属的既有根补写归属」还是「接回桌面决策窗」，否则升级用户会看到数据被移走。

自动化验证：`packages/services/test/data-root-host-drill.test.ts` 1 项全绿
（`tsx --test` 下 spawn 生产 Host，约 3s）。

## 24. 执行记录（2026-10-22，Web 单机回环 E2E）

`packages/server/test/web-remote-loopback-e2e.test.ts`（`pnpm --filter @mode/server test`）：
单进程内起发布形态 HTTP/WS 启动器（`createHttpServer` + `createLanAccessHost`），用真 LAN
配对码换设备令牌，再以该令牌连 `/ws`：

- `helloConversationV4` 必须返回 `clientMode=web-remote-replayable`、
  `deliveryProfile=replayable`；`clientHello` 完成后才算 `assertReady`；
- 订阅 `sessions-index`：假 agent 基座在 ACK **之前**推送 initial 帧，连接 facade 按 ACK
  归属把 pending 帧回放给订阅者（覆盖「响应之后的有主通知」时序）；
- replayable 连接上的 continuous-only 事件（CUA 权限观察）不得到达，但同一 workspace 的
  live sessions-index 帧继续投递——拒绝的是事件族，不是整条连接；
- `/ws` 在线数经 `onWebSessionChange` +1/-1 收回 0。

边界：用假 agent 基座（不起真 runtime/模型），验证的是传输与连接模式语义；真机双端与
`web-remote-replayable` 恢复链路的 UI 回归仍待 LR-5。

## 25. 执行记录（2026-10-22，UI tab 断连降级回归）

**结论先说**：降级代码已经在 `useRemoteWorkspaceHistory` 里（`onRemoteSessionClosed`
→ 清掉 tab 的 `remoteSessionId`、注销 session、把 running/streaming 任务收口为 failed、
记录断连原因），此前没有任何回归测试。本轮补纯逻辑用例：
`packages/ui/test/remoteWorkspaceSessionRuntime.test.ts`（4 项，`pnpm --filter @mode/ui test`）：

- active task + runtime running + meta running 同任务去重，只收口一次并带原因；
- 本地 runtime 已是终态时，滞后的 meta running 不得被改回 failed；
- optimistic running 参与收口；无运行任务返回 0；
- 同一 workspaceIdentity 的多个 tab 去重；不同 workspace 的同名 taskId 独立计数。

**UX 口径已定（2026-10-22，决策 B）并落地**：断连后有限自动重连，最多 3 次、退避
1s / 3s / 9s；干净退出与鉴权/版本类错误不重试；触底后停在断连态并 toast 一次，用户可手动重连。
启动阶段仍不自动连接（保留「打开应用不触发远端连接」的历史约束）。落点：

- `packages/ui/src/lib/remoteWorkspaceAutoReconnect.ts`：断连分类 + 调度器（纯逻辑）；
- `useRemoteWorkspaceHistory`：断连事件后调度；手动/Bot/移除 tab/卸载均取消；
  Bot 重连成功时还按 requestId 取消 UI 侧在途 connect；
- `RemoteSessionClosedEvent` 增加可选 `error`，Host 关闭事件的错误经桌面桥透传，
  用于区分「传输断开」与「鉴权/版本不兼容」；
- 规范：`docs/specs/remote-workspace-auto-reconnect.md`；
- 用例：`packages/ui/test/remoteWorkspaceAutoReconnect.test.ts`（6 项）+
  桌面桥错误透传断言。

真机（拔网/杀对端进程/断链后来回切）仍待 LR-5 验收。
