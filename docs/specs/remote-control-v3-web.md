# 远程控制 v3 Web 客户端

状态：2026-11-11 传输、账号、工作区隧道与远程桌面页面已落地；完整工作区页面与真机验收待补。母规范：
[remote-control-v3.md](./remote-control-v3.md)。

## 范围

浏览器作为 v3 Client：使用账号服务登录同账号设备，按 Control/Relay/LAN direct 选路，
在 Noise 通道内执行 allowlist RPC 或打开远程桌面/工作区隧道。

## 已实现

- `@mode/remote-core/web`：不依赖 `node:crypto` / `Buffer` 的浏览器密码学原语。
  - `generateWebIdentity` / `importWebIdentity` / `exportWebIdentity`：32 字节 X25519
    私钥的规范 base64url 持久化。
  - `WebNoiseHandshake` / `WebNoiseTransport`：Noise_IK_25519_ChaChaPoly_SHA256，
    线格式与 Node 实现逐字节一致。
  - 依赖 `@noble/curves`、`@noble/ciphers`、`@noble/hashes`。
- 互通测试：Node Initiator ↔ Web Responder、Web Initiator ↔ Node Responder 完成
  握手与双向 transport；错误 static key 被拒绝。
- `WebControlLink`：浏览器 Control WS（hello/connect/secure.handshake/ping）、
  WebRTC DataChannel / Relay WSS 数据面、Noise transport 与业务帧收发。
- 浏览器 WebRTC：支持 `RTCPeerConnection` 时默认宣告 `transport.p2p` 并优先协商
  （`p2p.offer/answer/candidate`），失败或 8s 超时回落 Relay；`p2p: false` 可强制
  只走 Relay。服务端仅当 Host 也宣告 `transport.p2p` 时才在 `connect.accepted`
  中确认 `p2pPreferred`，对端不支持时浏览器不会白等超时。未接 LAN direct。
- E2E：浏览器 `WebControlLink` → 真实 remote-server Relay → Node Host，完成
  `remote.echo` RPC。
- `WebAccountClient`：浏览器生成/持久化 X25519 设备身份；GitHub access token 换 Web
  会话、注册 Client 设备、401 自动 refresh 重试、409 轮换身份重试；设备目录只返回
  descriptor/在线状态/地址。凭据通过可注入的 `WebCredentialStore` 保存。
- 账号客户端测试：profile 复用、设备注册、401 刷新、目录投影。
- 浏览器 GitHub Device Flow：`WebAccountClient.begin/pollGithubDeviceLogin` 调服务端
  代理端点；GitHub access token 只在服务端换取账号会话，不返回浏览器。
- `/v3` 独立入口（`packages/web/src/v3/WebV3Page.tsx`）：服务地址、GitHub Device Flow
  验证码、设备目录、在线状态、退出、以及选择 Host 执行 `remote.host.info` 的加密连通
  测试。
- 默认凭据存储：优先 IndexedDB（`mode-remote-v3/kv`），不可用时回落 localStorage；
  登出删除整个 profile。
- `WebControlLink` 支持业务 `request()` 与 `openStream()`；`@mode/client` 的
  `openWebWorkspace` 经 `remote.http.request` 申请 capability，再打开 `/ws/host`
  的 `remote.ws` 流，包装成浏览器 `ISocket` 并复用 `SocketProtocol/ChannelClient`。
- `/v3` 页面提供“工作区隧道”连通测试（读取对端 `systemService.info`）。
- 远程桌面主控原语：`createWebRemoteDesktopSession`（`@mode/client`）经
  `remote.http.request` 调 Host 的 `/api/remote-desktop/session` 取双票据，再把
  `/ws/remote-desktop`、`/ws/remote-desktop-media` 两条 `remote.ws` 隧道包装成
  信令 `RemoteDesktopSignalSocket` 与媒体 `RemoteDesktopMediaSocket`；地址是
  `wss://v3-remote.invalid/...` 虚拟 URL，票据更新后按同一 URL 结构重开流。
  主控状态机与桌面 Host 共用 `controller.ts`（单会话互斥、握手窗口、bye/error 收口），
  媒体面复用 `nativeMediaSession`（新增 `openSocket` 注入点），不复制解码/重连逻辑。
- `/v3` 设备行新增「进入桌面」：`WebV3RemoteDesktop` 用平台子集
  （`RemoteDesktopPlatform`）注入现有 `RemoteDesktopSession`，媒体 socket 由同一
  adapter 注入；`VideoDecoder`/`crypto.subtle` 预检在建连前返回 `decoder_unsupported`。
  会话面复用画布、输入采集、剪贴板、画质切换与媒体级重连。
- 浏览器断线重连：Control/Relay 断开后保留 `WebRemoteConnection` 与 frame 监听，
  按 300ms→5s 退避重连（最多 10 次）；重连后重新 hello、建立连接、Noise 握手与
  Relay attach，原 connection 对象继续收发；在途业务帧不自动重放。E2E 覆盖 Host
  退出 → 浏览器重连 → Host 恢复 → 同一 connection 完成第二次 RPC。
- 工作区隧道级重连：`/ws/host` 隧道断开时重置 `SocketProtocol` 半包、让在途 RPC
  立即失败，再按 300ms→5s 退避重开（最多 10 次）；写不出去的业务帧有界排队。
  新 ChannelServer 的 Initialize 触发 `ChannelClient` 重放事件订阅，`services` 引用不变。
  测试覆盖「对端断开 → 自动重开 → 同一 services 完成第二次 RPC」。

## 待实现

1. 多账号隔离已实现：profile 按服务地址分键（`profile:<serverUrl>`），账号索引
   只存 serverUrl/deviceId/deviceName；旧单账号 key 首次读取自动迁移；`/v3` 登录前
   展示已保存账号并可切换。私钥/token 落盘用 AES-GCM 信封：密钥为不可导出的
   `CryptoKey`（同存 IndexedDB），旧明文读取兼容并在下次写入升级；
   localStorage 回落（无 IndexedDB）仍为明文。
2. 传输：P2P（浏览器 `RTCPeerConnection`）与 Relay WSS 已实现，选路顺序
   `p2p -> relay`；P2P 不可用或超时回落 Relay。LAN direct 仍未接（浏览器受混合内容
   限制，仅在 HTTPS 页面允许 `wss` 直连）。
3. UI：主 Web 已接通 v3 工作区（向导选 Host → 注册远程 session → 复用现有 tab/文件/
   终端/Git），见 [remote-control-v3-web-workspace.md](./remote-control-v3-web-workspace.md)；
   `/v3` 独立页仍只提供目录与连通测试。远程桌面页面已挂载，还缺真机验收
   （同意窗 → 首帧 → 键鼠输入 → 关闭释放）与弱网/断线复测。
4. 移动端复用同一 Web 客户端，尚未接入。
5. 安全：CSP 仍待；`/api/v1/*` 的 CORS 白名单已接（复用 `allowedOrigins` /
   `MODE_REMOTE_ALLOWED_ORIGINS`，未命中不回显 ACAO；未配置时仅同源可用）。
   私钥/token 已做静态加密信封（IndexedDB），不写入 URL 或日志；
   localStorage 回落路径仍是明文。

## 验收

- `@mode/remote-core/web` 与 Node Host 完成真实 Noise 握手并通过 Relay 收发 RPC。
- 刷新页面后身份与登录态恢复；登出后 IndexedDB 中无可用凭据。
- 浏览器 P2P 协商失败时回落 Relay，不静默重放业务请求。
- `pnpm --filter @mode/remote-core test` 覆盖 P2P 会话（offer/answer/超时/ICE 解析、
  分片拼包）；`packages/desktop/tests/web-remote-v3-p2p.test.mts` 用 werift 替代
  原生实现，验证浏览器链路 capability → 协商 → DataChannel → Noise → RPC。
- 与桌面 Host 的远程桌面/工作区共用同一 allowlist 与 `remote.ws` 隧道语义。
