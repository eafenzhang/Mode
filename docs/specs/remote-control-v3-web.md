# 远程控制 v3 Web 客户端

状态：2026-11-11 密码学原语落地，传输与 UI 未实现。母规范：
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
- `WebControlLink`：浏览器 Control WS（hello/connect/secure.handshake/ping）、Relay WSS
  attach、Noise transport 与业务帧收发；首期只走 Relay，不做 LAN direct。
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
- 浏览器断线重连：Control/Relay 断开后保留 `WebRemoteConnection` 与 frame 监听，
  按 300ms→5s 退避重连（最多 10 次）；重连后重新 hello、建立连接、Noise 握手与
  Relay attach，原 connection 对象继续收发；在途业务帧不自动重放。E2E 覆盖 Host
  退出 → 浏览器重连 → Host 恢复 → 同一 connection 完成第二次 RPC。

## 待实现

1. 多账号隔离与私钥加密存储；当前 profile 单账号保存，IndexedDB 中的私钥未额外加密。
2. 传输：Relay WSS 已实现；LAN direct 在浏览器受混合内容限制，仅在 HTTPS 页面允许
   `wss` 直连，否则回落 Relay。
3. UI：完整远程工作区页面（目录浏览、文件/终端服务）和远程桌面页面；当前 `/v3`
   只提供设备目录、allowlist RPC 与工作区隧道连通测试。
4. 移动端复用同一 Web 客户端，尚未接入。
5. 安全：CSP/Origin 校验、token 只存内存/安全存储、禁止把私钥或 access token
   写入 URL、localStorage 明文或日志。

## 验收

- `@mode/remote-core/web` 与 Node Host 完成真实 Noise 握手并通过 Relay 收发 RPC。
- 刷新页面后身份与登录态恢复；登出后 IndexedDB 中无可用凭据。
- 浏览器直连失败时回落 Relay，不静默重放业务请求。
- 与桌面 Host 的远程桌面/工作区共用同一 allowlist 与 `remote.ws` 隧道语义。
