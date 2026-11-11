# 远程控制 v3 重做执行计划

日期：2026-11-11。规格：[remote-control-v3.md](../specs/remote-control-v3.md)。
参考对象：`ds-harness-remote` 的公开协议与产品文档（MIT）；只借鉴架构与行为，不复制实现。

## 原则

- 不兼容 v2：不读写旧设备 token、不翻译旧凭据、不保留 `/d/<tunnelId>` 明文数据面。
- 单一所有者：账号/设备/membership/连接归账号服务；私钥与 pin 归设备；Noise/流归连接。
- 先协议后实现：线协议、限额、关闭语义、错误码先固化，再写运行时。
- 所有新增代码可独立测试；不接线到 UI 之前不删除旧 UI，但新产品路径不引用旧路径。

## 包与模块

```text
packages/remote-core
  src/domain/     协议帧、编码、限额、关闭语义、错误码、能力（纯函数）
  src/app/        Noise 会话状态机、分片、流多路复用、连接编排（无 IO）
  src/adapters/   node:crypto 原语、WebSocket 适配（可注入）
  src/contract.ts 对外类型与端口
  src/index.ts    对外工厂
  test/           单元 + 进程内双端测试

packages/remote-server
  src/domain/     descriptor/schema 校验、ticket 声明、连接状态
  src/app/        账号、设备、membership、连接用例
  src/adapters/   SQLite、HTTP、Control WS、Relay WS
  src/main.ts     独立服务入口
  test/           账号隔离、tokens、连接、撤销、E2E
```

## 阶段

### P0 协议与密码学（本次完成）

- `remote-core`：X25519 身份、Noise IK、secure channel、DSHF 分片、
  control/secure frame 编解码、stream mux、错误类型。
- 测试：握手、双向传输、篡改/重放/乱序、prologue 绑定、分片边界、大消息。

### P1 账号服务（本次完成）

- SQLite schema v1：accounts / web_sessions / devices / device_tokens /
  device_memberships / connections。
- GitHub Device Flow 验签接口（可注入 verifier；测试用 fake）。
- 设备注册、token 轮换、目录（无私密）、membership、连接创建、撤销。
- Control WS：hello、connect.request/incoming/accepted、secure.handshake、
  transport.selected、relay、ping/pong、关闭码。
- Relay WS：签名 ticket、attach/ready/detach、有界转发。
- 测试：跨账号拒绝、token 重放、撤销断连、目录不泄密、E2EE 中继无明文。

### P2 设备运行时（本次完成核心）

- `remote-core` 的 `HostConnection` / `ClientConnection`：
  - Control WS 连接与重连（full jitter）。
  - 发起/接受连接、Noise 握手、Relay attach、帧收发。
  - 流多路复用与 allowlist 路由端口。
- 进程内 E2E：两个设备注册到真实 server，完成握手，通过 Relay 加密 echo。
- LAN direct 的首帧/票据协议预留；实现直连适配器与选路策略。

### P3 产品接线（后续）

- Desktop Host：X25519 身份、注册、Control 常驻、业务 allowlist 适配器
  （Harness RPC、文件、终端、远程桌面信令/媒体）。
- Renderer：账号 UI、设备目录、连接状态、E2EE 状态展示。
- 删除 v2：cloud-relay 目录令牌与 `/d` 数据面、LAN Bearer、远控票据。
- Web 客户端 WebCrypto Noise（单独 spec）。

## 状态（2026-11-11）

- P0 完成：`packages/remote-core` 协议/身份/Noise/分片/会话/流复用，10 项单测。
- P1 完成：`packages/remote-server` 账号/设备/token/membership/连接、Control WS、
  Relay WS、签名 ticket、撤销断连；5 项测试（含 Control+Relay E2E）。
- P2 完成 LAN direct + Relay 双数据面与自动重连：Client 优先直连 Host `/ws/v1/direct`
  （ticket 首帧校验），失败回落 Relay；WebRTC / TURN / lease 恢复未落地，未宣告对应 capability。
- P3 部分完成：`packages/desktop/src/host/remoteV3/` 已接进 Host（设备身份、GitHub
  Device Flow、注册、Control 常驻、pin、RPC allowlist）；同安装可注册独立 Client
  角色并通过 `IRemoteV3Service.request` 走 E2EE 执行 RPC。
- 账号登录 UI 已重做：`ModeAccountDialog` → `RemoteV3AccountSection`（服务地址、
  Device Flow、在线状态、指纹、活动连接、只读设备目录、退出）；v2 云分区删除。
- 通用 E2EE WebSocket 隧道已完成：`remote.ws` stream ↔ 本机 LAN `/ws*`，
  Client 侧 `openTunnel/sendTunnel/closeTunnel/onTunnelEvent`；支持握手头；测试覆盖
  真实回声与头透传。无订阅者期间的事件会缓存，首个订阅者补齐 `open`/首帧。
- Host HTTP 桥已完成：`remote.http.request` ↔ 本机 `/api/*`，本地令牌注入、路径
  黑名单、2 MiB 限额；Client 侧 `httpRequest`。测试覆盖成功调用与 `/api/lan/*` 拒绝。
- 远程桌面 v3 已接通：远程控制页展示 v3 Host，`rdStartSession` 经 Host `openV3Session`
  走 HTTP 双票据 + 信令/媒体隧道 + 本机回环代理；测试覆盖信令与媒体回声。
- 远程工作区 v3 已接通：`RemoteTarget` 增加 `v3`，向导可选在线 Host，Host 侧
  `attachV3RemoteWorkspace` 经 HTTP 桥取 server-info/capability，再把 `/ws/host`
  E2EE 隧道包装成 `ISocket` 复用 LAN 的 RPC 链路；测试走真实 HTTP/WS 服务端，
  覆盖 capability 头、隧道帧格式与 `Initialize` 时序。
- Control 链路自愈已完成：Host 非主动断链后 full-jitter 指数退避重连（500ms–30s），
  401/403 视为凭证失效不重试；Client 断链立即失败同设备在途 RPC；心跳增加 pong 看门狗。
  测试用「同设备第二条 Control 顶掉旧连接」验证 Host 自动恢复并继续响应加密 RPC。
- 账号切换已完成：配置时对比 `/auth/me` 与本地 accountId，变化或 409 `device_conflict`
  时轮换 Host/Client 身份并清除旧 token；测试覆盖 A 账号 → B 账号后 deviceId 变化、
  新账号目录不含旧设备、状态仍 online。
- 远程桌面媒体隧道重连已落地：媒体流断开后保留 signal 会话与回环 URL，renderer 用
  新 `mediaTicket` 重连时按票据重开 `remote.ws` 媒体流。
- 工作区 `/ws/host` 隧道级重连已落地：稳定 ISocket 重开隧道；`SocketProtocol.reset`
  清半包，`ChannelClient.notifyTransportClosed` 拒绝在途 RPC，新 Initialize 重放事件
  订阅。测试覆盖传输断开后同一 services 继续完成 RPC。
- v2 账号面已下线：`ICloudAccountService`、Host v2 账号服务、账号 UI 分支与对应测试删除；
  侧边栏/账号弹窗只认 OAuth 用户与 v3 账号。`packages/cloud-relay` 的目录令牌/`/d`
  数据面、LAN Bearer、远控票据仍被 LAN 路径使用，待 LAN 替代后再删。
- 生产 TLS 已支持：`startRemoteServer({ tls })` 或 `MODE_REMOTE_TLS_KEY_FILE` /
  `MODE_REMOTE_TLS_CERT_FILE`；URL 与 Relay 自动使用 https/wss，`/health` 与 API 走同一
  TLS 监听。反向代理仍可作为可选入口。
- Web Noise 原语已落地：`@mode/remote-core/web` 提供浏览器 X25519 + Noise IK，
  与 Node 实现双向互通。`WebControlLink` 已通过 Relay 与 Node Host 完成 RPC；
  `WebAccountClient` 已覆盖身份、设备注册、token 刷新、目录与 GitHub Device Flow
  （经服务端代理，GitHub token 不进浏览器）；`WebControlLink` 已支持 Control/Relay
  断线自动重连并保持同一 connection 引用，也支持业务 `request/openStream`。
  `openWebWorkspace` 已把 `/ws/host` 隧道包装成浏览器 ChannelClient；`/v3` 页面提供
  IndexedDB 凭据、登录、设备目录、加密 RPC 与工作区隧道连通测试。完整工作区/远程桌面
  Web 页面仍待实现。
- Web 远程桌面主控原语已落地：`createWebRemoteDesktopSession` 走 HTTP 桥创建会话，
  信令/媒体分别用 `remote.ws` 隧道承载；主控状态机抽到 `@mode/client` 并支持注入
  socket（桌面 Host 仍传 `ws`），媒体面新增 `openSocket` 注入点，解码/重连逻辑未复制。
- Web 远程桌面页已接通：`/v3` 设备行「进入桌面」→ `WebV3RemoteDesktop`，用
  `RemoteDesktopSession` + 平台子集注入复用画布/输入/剪贴板/画质切换；发起前做
  WebCodecs/WebCrypto 预检（`decoder_unsupported`）。真机验收未执行。
  完整工作区页面（目录浏览/文件/终端）仍待实现，规格与工作量见
  [remote-control-v3-web-workspace.md](../specs/remote-control-v3-web-workspace.md)（约 3–4 轮）。
- Web 工作区 v3 已接通：`connectRemote({kind:"v3"})` 复用 `/v3` 已存凭据建 Noise
  连接，`attachV3WorkspaceSession` 把 `/ws/host` 注册进
  `remoteWorkspaceSessionStore`（v3 与 LAN/SSH/Docker 共用同一注册表）。主 Web 注入
  浏览器 Client 版 `remoteV3Service`（目录/Device Flow/按 Host 复用连接的
  request·httpRequest·tunnel），向导 v3 步骤与账号弹窗因此可用；
  `allowRemoteWorkspace` 在 LAN 主控或存在 v3 账号时放开。真机双端验收未执行。
- Web 工作区隧道级重连已落地：`/ws/host` 断开时 `SocketProtocol.reset` + 在途 RPC
  立即失败，按 300ms→5s 退避重开；新 Initialize 重放订阅，`services` 引用不变。
- 跨账号配对 v1 服务端已落地：ticket → claim（8 位码）→ Host 确认 → 双端换凭证，
  建 device_only 账号 + 单条 membership，Host `inbound_only` / Client
  `pairing_restricted`；次数/过期/重复使用均有错误码。规格见
  [remote-control-v3-pairing.md](../specs/remote-control-v3-pairing.md)，
  浏览器 Client SDK（claim/status/redeem/cancel）与 `IRemoteV3Service` 配对方法已接：
  桌面 Host 实现 begin/confirm/get/cancel（确认后轮换凭证并重建 Control），浏览器实现
  claim/get/redeem/cancel；E2E 覆盖「桌面 Host × 浏览器 Client」全流程。
  账号弹窗已挂配对面板（短票 + 本机验证码），状态机为纯函数并单测。
- 仍未开始：WebRTC/P2P/TURN。
- 账号展示已接入：`/auth/me` 展示名落库并进入 Host 状态，侧边栏/账号弹窗优先显示
  v3 账号名，退出菜单直连 `disconnectAccount`。
- P2 直连前置已落地：Host 注册上报 LAN 地址（≤8）并进入设备目录，远程控制页展示
  首个地址；`remote.workspace.list` 接到真实工作区列表。LAN direct 数据面/选路仍待实现。
- 测试：74 项（core 18 + remote-server 19 + desktop Host 5/工作区 1/隧道 1/媒体重连 1/
  配对 E2E 2 + 配对 UI 状态机 3 +
  ChannelClient 重连 1/Web 工作区 1/Web 工作区重连 1/Web 远程桌面 5/v3 工作区 attachment 2/
  Web v3 服务 6 +
  server direct WS 2 + UI v3 目标/向导 3 + 侧边栏账号 2）。
- 门禁：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、
  `pnpm knip:gate` 全绿；新包测试已加入 `.github/workflows/release.yml` verify，
  Desktop Host 测试由既有 desktop tests glob 覆盖。

## 验收命令

```sh
pnpm --filter @mode/remote-core test
pnpm --filter @mode/remote-server test
pnpm typecheck
pnpm lint
pnpm architecture:check --changed
```
