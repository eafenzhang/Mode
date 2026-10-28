# Spec：Web / fpk 浏览器远程桌面主控（P3）

状态：2026-10-28 已实现中继链路（真机双端画面/输入验收待执行）。关联：
[web-remote-workspace.md](./web-remote-workspace.md)（LAN 配对/端点）、
[lan-remote-desktop.md](./lan-remote-desktop.md)（产品与信令协议）、
[lan-remote-desktop-native-media.md](./lan-remote-desktop-native-media.md)（媒体面/加密）。

## 1. 问题

现有远程桌面主控链路是 **桌面 Host → 对端**：Host 用凭据服务里的对端令牌请求会话，
拿双票据后由 main 连信令 WS、renderer 用 `nativeMediaSession`（WebCodecs + WebCrypto）
连媒体 WS。Web/fpk 没有这条 Host 链路，且浏览器不能持有对端长期令牌，也不能在
https 页面直连对端的 `ws://` 媒体地址。

## 2. 方案：NAS 服务端中继

```text
浏览器（Web/fpk）
  ├─ POST /api/remote-desktop/peer-session {serverId, quality, codec}
  │     → server 用凭据令牌 POST 对端 /api/remote-desktop/session（同意窗最长 90s）
  │     → 拿信令/媒体双票据，server 连对端信令 WS + 媒体 WS
  ├─ WS /ws/remote-desktop-peer/<id>/signal   （JSON 信封：state / signal）
  ├─ WS /ws/remote-desktop-peer/<id>/media    （下行二进制帧；上行 JSON 输入）
  └─ 现有 useRemoteDesktopSession + nativeMediaSession：解密、WebCodecs 解码、绘制、输入
```

- 对端长期令牌只留在服务端凭据服务，不出现在任何浏览器响应。
- 媒体帧保持对端加密（aes-256-gcm；key/salt 经信令下发），服务端**不解密、不转码**，
  只做 WebSocket 字节转发；浏览器用现有 `nativeMediaSession` 解密/解码。
- 输入事件（`pm/md/mu/mw/kd/ku/tx/clip` 等 JSON）由浏览器媒体 WS 上行，服务端原样转发
  到对端媒体 WS；不解析业务字段。
- `media-open` 等信令原样经浏览器信令 WS 下发；现有 reducer 不需要改。

## 3. 所有者与状态

| 状态 | 所有者 | 位置 |
| --- | --- | --- |
| 对端令牌/端点 | `ICredentialService`（与 P2 共用） | `lan:peer:*` |
| 主控会话（对端信令/媒体 WS、浏览器 WS） | 服务端 `RemoteDesktopPeerController` | 进程内 `sessions` |
| 浏览器活动会话 id/信令 WS | Web 平台 `activeRemoteDesktopSession` | 浏览器内存 |
| 解码/画布/输入 | `useRemoteDesktopSession` + `nativeMediaSession` | 浏览器 |

- 一个浏览器同时只有一个活动远程桌面会话；`rdStartSession` 先收口旧会话。
- 服务端会话在浏览器信令 WS 关闭、对端 bye/error、或 `POST .../end` 时收口；
  浏览器媒体 WS 断开不立即杀对端媒体（允许 UI 重连到同一中继），无浏览器 60s 后回收。
- 浏览器信令/媒体 WS 都走既有 `/ws/*` 令牌鉴权（页面 Cookie）。

## 4. 协议

### 4.1 HTTP

```text
GET  /api/remote-desktop/peer-capability?serverId=...
  -> { online, remoteDesktop, remoteDesktopNative, name?, endpointKind?, endpointHost? }
POST /api/remote-desktop/peer-session
  body { serverId, quality?, codec? }
  -> { ok: true, sessionId, signalPath, mediaPath } | { errorCode?, error }
POST /api/remote-desktop/peer-session/:id/end
  body { reason?: "user-end" | "error" }
```

### 4.2 浏览器信令 WS（`signalPath`）

- 服务端 → 浏览器：
  `{"kind":"state","state":"connected"|"closed","error?","retryable?"}`
  `{"kind":"signal","message":<RemoteDesktopSignalMessage>}`
- 浏览器 → 服务端：`{"kind":"signal","message":<RemoteDesktopSignalMessage>}`
- 服务端收到浏览器消息后校验 `remoteDesktopSignalMessageSchema`，合法才转发给对端；
  对端消息同样校验后转发。非法消息丢弃并计数，不结束会话。

### 4.3 浏览器媒体 WS（`mediaPath`）

- 下行：对端媒体 WS 的二进制帧原样转发。
- 上行：浏览器 JSON 文本原样转发到对端媒体 WS；二进制上行不支持。
- 服务端不缓存帧；浏览器断开后保留对端媒体 WS 最多 60s，期间新浏览器媒体 WS 可继续接流。

## 5. Web 平台映射

| 方法 | 实现 |
| --- | --- |
| `rdGetPeerCapability(serverId)` | GET peer-capability；失败按 `online:false` 收口 |
| `rdStartSession(serverId, quality, codec)` | 先做本机 WebCodecs 预检（缺失 → `{error:"decoder_unsupported"}`，不发请求）→ POST peer-session → 打开信令 WS → 返回 `{ok:true, mediaUrl: mediaPath 的 wss 地址}` |
| `rdSendSignal(message)` | 信令 WS 发 `{kind:"signal",message}`；未连接时丢弃 |
| `rdEndSession(reason)` | 先发 bye，再 POST end，关闭信令/媒体 WS |
| `rdGetControlledState()` | Web 端恒为 `{controlled:false}`（主控端不是被控端） |
| `onRemoteDesktopSessionEvent(handler)` | 浏览器内 emitter；WS 信封拆成 state/signal |
| `rdReadClipboardText()` | `navigator.clipboard.readText()`；权限拒绝返回空串 |

## 6. 失败语义

| 场景 | 行为 |
| --- | --- |
| 未配对/令牌失效 | POST 返回 `unauthorized`；UI 显示重新配对 |
| 对端拒绝/超时 | 返回 `consent_denied`/`consent_timeout`/`disabled`/`busy` |
| 对端离线 | `internal` + 可读错误；不进入 connecting 死等 |
| 对端信令断开 | 服务端向浏览器发 `closed{retryable:true}`，UI 走会话级重连 |
| 浏览器信令 WS 断开 | 服务端收口对端会话；UI 页面重连时重新发起 |
| 浏览器媒体 WS 断开 | 服务端保留对端媒体 60s；UI 重连到同一 mediaPath 继续 |
| 浏览器页面刷新 | 旧会话由信令 WS close 收口；不跨页面恢复远程桌面 |
| 浏览器不支持 WebCodecs | 本机预检（`typeof VideoDecoder !== "function"`）在发起前返回 `decoder_unsupported`：不请求对端、不弹同意框，文案提示改用最新版 Chrome/Edge |
| 解码器运行期失败 | 沿用 `nativeMediaSession` 失败归因（HEVC 降级一次后仍失败即收口） |

## 7. 安全边界

- 对端令牌、信令 ticket、媒体 ticket 均只在服务端内存/凭据中；浏览器只拿到 sessionId
  与路径。
- 中继 WS 使用既有 LAN/网关令牌鉴权；未授权 401。
- 媒体帧保持端到端加密（服务端无法读取画面）；key/salt 经信令下发，服务端同样可见但
  不落日志、不持久化。
- 服务端只转发已通过 schema 校验的信令；输入事件按原样字节转发（协议层已由对端校验）。
- 远程桌面仍需对端本机同意（被控端同意窗）；Web 端不绕过。

## 8. 实现记录（2026-10-28）

- `remoteDesktopPeerClient.ts` 迁到 `packages/server/src/remote/`（desktop 兼容 re-export），
  对端能力探测/会话请求与 desktop Host 共用一份。
- `RemoteDesktopPeerController`（`packages/server/src/remote/remoteDesktopPeerController.ts`）：
  - `start()` 用凭据令牌请求对端会话、等待对端信令 WS open，建立会话；
  - 对端信令消息 schema 校验后按 `{kind:"signal"}` 转发，浏览器信令按同一 schema 校验后
    转发对端；`media-ticket` 更新下次媒体打开地址；
  - 对端媒体 WS 在浏览器媒体 WS 接入时打开，二进制直转、文本输入直转；
  - 浏览器媒体先于对端媒体发送的 `ready` 等文本入队，对端 open 后按序补发；
  - 浏览器信令 WS 60s 未接入、媒体无浏览器 60s、bye/error/关闭均收口对端连接。
- HTTP/WS：`/api/remote-desktop/peer-capability`、`peer-session`、
  `peer-session/:id/end`、`/ws/remote-desktop-peer/:id/{signal,media}`；全部走既有鉴权。
- Web 平台实现 `rdGetPeerCapability`、`rdStartSession`、`rdSendSignal`、`rdEndSession`、
  `onRemoteDesktopSessionEvent`、`rdReadClipboardText`；`rdStartSession` 返回 NAS 中继
  `mediaUrl`，现有 `useRemoteDesktopSession`/`nativeMediaSession` 无改动直接复用。
- 本机解码能力预检：`rdStartSession` 在发请求前检查 `VideoDecoder`；缺失时新增错误码
  `decoder_unsupported`（与对端 `unsupported` 区分）直接收口到会话关闭层。
- 已验证：`remote-desktop-relay.test.ts` 用 fake 对端信令/媒体 WS 完成
  浏览器 hello → 对端、对端 monitors → 浏览器、浏览器 `ready`/输入 → 对端、
  对端二进制帧 → 浏览器、关闭收口；无配对会话返回 401、能力探测离线收口。
  **待真机**：Windows 对端同意 → 浏览器首帧 → 键鼠输入 → 关闭释放。
- 已知限制：浏览器需支持 WebCodecs（Chrome/Edge/Safari 现代版本；H.264 由对端默认回退）；
  页面刷新结束远程桌面会话；浏览器媒体断开 60s 后释放对端媒体，超时后的媒体重连需重新
  发起会话（UI 的媒体级退避上限约 14s，落在 60s 窗口内）；服务端中继不解密，媒体加密
  仍由对端 key/salt + 浏览器 WebCrypto 完成。

## 9. 验收

1. 两台设备配对后，Web 设备列表显示「进入桌面」；点击进入 → 对端同意 → 浏览器出首帧。
2. 键鼠输入在浏览器内操作对端；画质档位切换可重建会话。
3. 关闭页面/离开会话：对端收到 bye/WS close，无残留会话。
4. 对端断网：浏览器进入明确失败/可重连态，不无限等待。
5. 未配对/令牌失效：返回 unauthorized，不泄露令牌。
6. 服务端单测：信令信封转发/校验、媒体双向字节转发、会话收口与 60s 回收。
