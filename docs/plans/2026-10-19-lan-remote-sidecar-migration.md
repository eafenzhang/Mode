# LAN / 远控迁移到 Tauri Host sidecar（独立立项）

状态：2026-10-19 立项，未开工。
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

- [ ] 更新 `tauri-renderer-host.md`：新增 LAN/RD 频道、local-only 集合、attachment 契约。
- [ ] 更新 `tauri-platform-commands.md`：登记本立项；明确 `window.mode` 与平台委托的关系。
- [ ] 新增频道 descriptor 与类型；补静态契约测试（频道名、命令名、local-only 断言）。
- [ ] 建立假 credentials/settings/startServer 的测试夹具。

验收：`pnpm typecheck`、静态契约测试红/绿各一次；没有产品行为变化。

### LR-1 LAN 管理面（2–3 天）

- [ ] Host adapter：复用 `lanAccessServer`、`lanRemoteAttach`、`desktopLanDiscovery`；
      显式 dispose；动态 import 以绕开 stdout 守卫。
- [ ] 注册 local-only `lan-access` 频道到 `host-standalone`；启动时按
      `lanAccessEnabled` 恢复监听，失败只告警并回 disabled。
- [ ] `IServiceAccessor.lanAccessService` + `RemoteServiceAccess` 代理。
- [ ] `desktopPlatform.ts` 的 11 个 LAN 方法委托 Host 服务，保留 `window.mode` fallback。
- [ ] 修复 `removeLanPairedPeer` / `renameLanPairedPeer` 的静默 no-op。
- [ ] 测试：disabled state、enable/disable、pair code 一次性、paired peers 排序、
      workspaces、rename/remove、local-only 不被 peer 集合看到、事件订阅。
- [ ] 真机：设置页 LAN 卡片、发现、配对、已配对设备管理、工作区目录读取。

验收：Tauri 产品 UI 不再出现「读取已配对设备失败」；LAN 开关与配对闭环可重放。

### LR-2 远程桌面设置面（1–2 天）

- [ ] Host adapter：`remoteDesktopServer` 的 server state / enable / authorize / revoke。
- [ ] `remote-desktop` 频道；设置页与 LAN client 记录仍由同一所有者写。
- [ ] `rdGetPeerCapability` / `rdGetControlledState` 接 Host 或壳能力。
- [ ] 测试：server state、授权三态、peer capability、关闭门控。

验收：设备页能列出已配对设备并显示能力；被控开关与逐设备授权可用。

### LR-3 远程桌面会话（1–2 周）

- [ ] 把 controller/coordinator 从 Electron main 抽到 Host（保留状态机与单会话互斥）。
- [ ] 信令/媒体事件经 `onDynamicSessionEvent` 到 renderer；不缓冲旧信令。
- [ ] 壳能力：`rdReadClipboardText`、显示器枚举/几何、光标位置。
- [ ] 同意窗/指示条：推荐 Rust `WebviewWindow`；60s 超时、拒绝、结束按钮。
- [ ] 窗口关闭 / 应用退出：先发 bye/end-session，再收 Host/sidecar。
- [ ] 测试：controller WS、coordinator 状态机、media relay、clipboard gate、退出收口。
- [ ] 两台 Windows 实机验收（主控/被控、授权、多屏切换、剪贴板、断连重连）。

验收：`in-app-remote-desktop.md` 的实机清单通过；旧对端仍按能力位降级。

### LR-4 远程工作区 attachment（1–2 周）

- [ ] Spike：单端口多路复用 vs 页面多端口 broker；给出跨语言改动量、失败模式、测试方案。
- [ ] 选定方案后实现 attachment broker：local + N 个 remote scope 的路由与回收。
- [ ] `windowRemoteConnectionRegistry` 进 standalone Host；`connectRemote`/cancel/bind/dispose。
- [ ] scoped services attachment 到 renderer；`remoteSessionId + workspacePath + workspaceIdentity`
      三元组严格校验。
- [ ] 事件：connection log、closed、bot reconnect；UI tab 降级与重连。
- [ ] 测试：connect/cancel/bind/dispose、attachment 路由、Host 退出、stale session、
      `REMOTE_WORKSPACE_IDENTITY_MISMATCH`、断线重连。

验收：LAN 已配对工作区可重连；对话/任务/文件/Git 走 remote scope；关闭 tab 释放 session。

### LR-5 验收与发布（2–3 天）

- [ ] 两台 Windows 实机：LAN 发现→配对→attach→prompt→流式结果。
- [ ] 远程桌面双向授权、剪贴板、多屏、断连重连。
- [ ] 手机/Web 远控复用 Host attachment 的回归（`web-remote-replayable`）。
- [ ] Tauri 产品 UI E2E：设置页、设备页、连接向导、工作区重连。
- [ ] 逐项移除 `window.mode` 的 rejecting/no-op 条目，并更新注入测试的期望集合。
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
