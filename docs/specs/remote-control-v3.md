# 远程控制 v3：设备身份、端到端加密与账号设备目录（破坏性重做）

状态：2026-11-11 立项。**本 spec 取代 v2 的远程控制与 Mode Cloud 账号方案**：
v2 的「同账号即信任 + 目录下发被控端访问令牌 + 可信自建明文中继」整体退役，
不做旧端点、旧令牌、旧凭据格式兼容。参考 `ds-harness-remote` 的公开协议与产品设计
（Noise IK、Control/Relay 双平面、LAN/P2P/TURN/Relay 选路、设备身份固定、
同账号 membership），实现按本仓库 clean-room 纪律重写。

关联：[clean-room-policy.md](./clean-room-policy.md)、
[cloud-personal-accounts.md](./cloud-personal-accounts.md)（v2，退役）、
[wss-relay-mvp.md](./wss-relay-mvp.md)（v2，退役）、
[remote-control-device-model.md](./remote-control-device-model.md)（UI 设备模型继续复用）。

## 1. 决策与范围

### 1.1 一句话

远程控制不再把「被控端本机 API 令牌」当网络凭证，也不再把中继当可信明文终点。
Host 与 Client 各自持有独立 X25519 设备身份；账号服务只做账号、设备目录、在线状态、
membership、连接授权、信令与密文中继；业务数据只在两端之间以 Noise IK 端到端加密。

### 1.2 破坏性变更

- **删除**目录下发 `accessToken`：账号服务不再保存或返回任何被控端本机令牌。
- **删除**以 `Authorization: Bearer <设备本机令牌>` / `?token=` / `mode_lite_token`
  Cookie 作为公网远程访问凭证的语义；这些只允许在 Host 本机回环内使用。
- **删除** `/d/<tunnelId>` 明文透明中继作为远程控制数据面；中继只转发密文。
- **删除** v2 的设备 `deviceToken` 与 `relayToken` 混合模型；改为短 access +
  可轮换 refresh 的设备凭证，以及每连接签名票据。
- **删除**「同账号设备零确认直连」的隐式信任；改为账号 membership + 本地 identity
  pinning 双层校验。
- 旧客户端不兼容：升级后必须重新登录并注册设备身份；不迁移旧凭据。

### 1.3 非目标（v3 首期）

- 组织/团队/RBAC/SSO/计费。
- 跨账号扫码配对（配对 v1 在 §10 定义，首期只实现同账号；跨账号后续独立迁移）。
- 远程桌面像素编码算法本身（继续复用现有媒体管线，只替换其承载与信任边界）。
- 浏览器 Web 端 Noise 实现（Web 客户端后续单独适配；首期客户端为 Desktop/CLI）。

## 2. 威胁模型与信任边界

| 角色 | 可以知道 | 不能知道 |
| --- | --- | --- |
| 账号服务 | 账号、设备 ID/角色/平台/公钥、在线状态、连接与流量规模、信令 | Prompt、会话、工具输出、工作区路径、文件内容、终端输入输出、Noise 明文与密钥 |
| 中继节点 | 连接 ID、设备对、密文长度/方向/时序 | 同上；密文不得落盘或写日志 |
| TURN | 五元组与包大小 | Noise 明文；WebRTC 之上仍有 Noise |
| 被授权 Client | 授权范围内的 Host 能力 | 未授权路径、其他账号设备、Host 私钥 |

信任假设：账号服务是首次身份分发的信任源。服务端被攻破可能首次注入伪造设备公钥；
本地 pinning 只能阻止已知设备换钥，不能阻止首次替换。此限制必须在 UI/文档中如实表达。

## 3. 身份模型

### 3.1 设备身份

每个 Host 与 Client **角色**各自拥有：

```text
deviceId       UUIDv7/ULID；账号服务与本地 trust store 的稳定标识
role           host | client
platform       desktop | cli | web | android | ios | ...
identityKey    X25519 公钥，32 字节规范无 padding base64url（43 字符）
```

- 私钥只在本机生成与保存，永不上传。
- 同一安装的 Host 与 Client 角色不得复用 `deviceId`、私钥或设备凭证。
- 设备描述（descriptor）只含上述字段 + `name` + `clientVersion?` + `hostVersion?`。
- 账号服务拒绝同一 `deviceId` 对应不同 `identityKey`、`role` 或 owner。
- 本地 trust store 键：`serverOrigin + role + peerDeviceId → identityKey`。
  已知 `deviceId` 换钥默认拒绝；换钥必须由用户显式移除旧 pin 后重新授权。

### 3.2 两层授权

一条连接必须同时通过：

1. **账号 membership**：账号服务只允许同账号、双方未撤销、角色互补的 Host/Client。
2. **本地 identity pinning**：双方用对方 descriptor 的 `identityKey` 完成 Noise IK，
   并与本地 pin 比较；不一致 fail closed。

Host 收到 `connect.incoming` 后，必须用自己的设备凭证调用账号服务重新读取 Client
descriptor，交叉校验账号、角色、公钥和 membership。仅信令事件本身不构成授权。

## 4. 账号与凭证模型

### 4.1 账号

- 身份来源：GitHub OAuth Device Flow（沿用产品既有登录方式；不带入 v2 令牌）。
- 账号锚点：`HMAC-SHA256(pepper, "github:" + githubUserId)`；JIT 建号。
- Web 会话：32 字节随机 token，服务端只存 SHA-256，默认 30 天，轮换与撤销。

### 4.2 设备凭证

设备注册成功后签发：

```text
accessToken   短 TTL（默认 1 小时）的不透明 token；服务端只存 SHA-256
refreshToken  长 TTL（默认 30 天）的可轮换 token；服务端只存 SHA-256
```

- refresh 轮换：旧 refresh 立即失效；同一 refresh 重复使用视为重放，撤销整条
  token family。
- 设备 access token 只用于账号服务 REST/Control WS 鉴权，**不能**访问其他设备、
  不能兑换 Web 权限、不能作为 Host 本机 API 凭证。
- 设备撤销同时撤销 token family、membership 与活动连接。

### 4.3 Membership

- 设备注册时，账号服务为该设备与同账号所有未撤销的**异角色**设备建立 membership。
- `host ↔ client` 是唯一有效组合；同角色不建立。
- 撤销单条 membership 只断开该设备对，不影响设备与其他设备的关系。
- 设备重新注册可恢复被撤销的派生 membership；显式撤销的 pair 需要重新授权
  （v3 首期：同账号重新注册即恢复；§10 配对 v1 不自动恢复其他 pair）。

### 4.4 状态所有者

| 状态 | 唯一所有者 |
| --- | --- |
| 账号、Web 会话 | 账号服务 SQLite |
| 设备 descriptor、公钥、角色、撤销 | 账号服务 SQLite |
| 设备 access/refresh token 哈希 | 账号服务 SQLite |
| membership | 账号服务 SQLite |
| 连接记录与关闭原因 | 账号服务 SQLite / 进程内 registry |
| 设备私钥、本地 pin | 设备本地凭据存储 |
| Noise 会话、流状态 | Host/Client 进程内，按 connectionId 隔离 |
| 账号 UI selected host / 草稿 | Renderer/UI 本地状态，不是服务端事实 |

## 5. 网络平面

### 5.1 三个平面

```text
REST      HTTPS：登录、设备注册/刷新/撤销、设备目录、membership、连接创建
Control   双方各自出站 WSS：hello、connect.*、secure.handshake、relay、ping/pong
Data      LAN 直连 WSS 或 Relay WSS / WebRTC DataChannel：只承载 Noise 密文
```

Control frame 是服务端可读 JSON，**不得含业务明文**。业务明文只存在于 Noise 通道内。

### 5.2 选路

目标顺序：

```text
LAN direct (WS + Noise)  ->  Relay (WSS + Noise)
```

- LAN direct 已落地：双方宣告 `transport.lan`，Host 上报 LAN 地址；Client 先尝试
  `/ws/v1/direct`，失败/超时回落 Relay。WebRTC / TURN 未实现前不得广告对应 capability。
- LAN 服务目前是明文 WS；直连内容仍由 Noise 加密，ticket 与连接元数据在局域网可被观测，
  这是当前 LAN 监听没有 TLS 的已知限制（WSS 待后续）。
- Host 在设备目录上报 LAN 地址（`addresses`，最多 8 个 `host:port`），直连选路使用。
- Relay 分配独立 data WebSocket；双方用服务端签名 ticket `relay.attach` 后转发密文。
- `transport.selected` 在 Noise 握手前由 Client 发送并经 Control 转发；Host 必须按
  消息顺序应用，不能在握手后切换已绑定数据面。
- P2 增加 LAN direct 后，Client 先尝试 `/ws/v1/direct`，失败/超时回落 Relay；
  WebRTC/P2P/TURN 再以 `transport.webrtc.v1` capability 增加。

### 5.3 降级与恢复

- 连接建立期：LAN 失败 → Relay；Relay 失败 → 关闭连接并报错，不静默重放业务请求。
- 已建立连接：DataChannel/Relay 断开 → 关闭该逻辑连接，清理 Noise、RPC、流与审批；
  Client 重新执行授权、选路和 Noise 握手。v3 首期不做跨 Control 中断的 lease 恢复。
- 断线重连使用 full jitter：`[0, cap]`，`cap` 默认 30s；显式撤销/替换/身份变化不重试。

## 6. 线协议

### 6.1 编码

- JSON UTF-8；时间 Unix 毫秒；二进制无 padding base64url。
- Control frame 上限 64 KiB；Relay frame 上限 1 MiB；业务分片完整上限 4 MiB。
- 严格 schema：未知字段、重复属性、非法长度、非规范 base64url 一律拒绝。

### 6.2 Control frame

```json
{ "v": 1, "id": "ULID", "type": "hello", "timestamp": 0, "payload": {} }
```

类型：

```text
hello / hello.ack
connect.request / connect.incoming / connect.accepted
transport.selected
secure.handshake
relay / relay.attach / relay.ready / relay.detach
ping / pong
error
```

关闭码：

```text
4001 服务端关闭/心跳超时（可重试）
4002 设备凭证失效（刷新后重试）
4003 连接被替换（不重试）
4004 授权撤销（不重试）
4008 非法帧/协议错误（不重试）
4009 服务端停机（可重试）
4011 协议版本不兼容（不重试）
4012 恢复票据被拒（不重试）
```

### 6.3 Hello

```json
{
  "v": 1, "type": "hello",
  "payload": {
    "role": "host",
    "deviceId": "...",
    "accessToken": "...",
    "protocols": [1],
    "clientVersion": "0.0.1",
    "capabilities": ["transport.lan", "transport.relay", "secure.noise-ik.v1"]
  }
}
```

`hello.ack` 返回协议版本、`connectionSessionId`、心跳间隔、frame 上限、双方能力交集。
Server 不得返回 Client 未声明的 capability。

### 6.4 连接建立

```text
Client  connect.request {hostDeviceId, preferredTransports}
Server  校验 membership/账号一致/角色/在线 → 创建 connectionId
Host    connect.incoming {connectionId, clientDescriptor, authorization:"account",
                          relay:{endpoint,ticket,nodeId,epoch}?, directTicket?}
Client  connect.accepted {connectionId, hostDescriptor,
                          relay:{...}, directTicket?}
Host    用设备凭证 GET /api/v1/devices/{clientDeviceId} 交叉校验 + 本地 pin
Client  发送 transport.selected {transport:"lan"|"relay"}
双方    经 Control 交换 Noise IK step 1/2（opaque）
双方    Relay attach / LAN data socket ready
        ===== Noise transport ciphertext =====
```

- 同一 Host 允许多个不同 `clientDeviceId` 的独立连接；同一 Client 新连接替换旧连接。
- Server 转发同一 connection 的 `transport.selected` 与 `secure.handshake` 必须保序。
- Host 在 transport 尚未确定时暂存 handshake，不得猜测数据面。

### 6.5 Secure channel

```text
Noise_IK_25519_ChaChaPoly_SHA256
Initiator = Client, Responder = Host
Prologue  = "MODE-REMOTE-V3" || u16(version) || len-prefixed(connectionId)
            || len-prefixed(hostDeviceId) || len-prefixed(clientDeviceId)
```

- 双方 static key 来自受 membership 保护的 descriptor，并与本地 pin 比较。
- handshake step 只允许 1/2，`data` 为规范 base64url。
- 每方向独立 64-bit nonce；认证失败、重复、旧消息、乱序、连接不匹配 fail closed。
- Noise 单消息上限 65,535 字节。编码后业务消息 > 48 KiB 时先切 DSHF 分片：
  `DSHF` magic、u8 version、u32 messageId、u16 index、u16 total、u32 totalBytes、
  payload ≤48 KiB；完整消息 ≤4 MiB，并行重组 ≤8。
- v3 首期不做 rekey；达到实现阈值或长连接需要时重建连接。

### 6.6 Relay

独立 Relay data WS：`wss://<server>/ws/v1/relay`。首帧：

```json
{ "v":1, "type":"relay.attach",
  "payload": { "connectionId":"...", "deviceId":"...", "peerDeviceId":"...",
               "ticket":"<base64url>" } }
```

ticket = HMAC-SHA256(serverSecret, canonical payload)，payload 绑定
`connectionId + 设备对 + nodeId + epoch + aud + exp`。成功后 `relay.ready`；
之后只接受目标匹配的 `relay` frame。任一端断开 → 对端 `relay.detach`。
Relay 节点不解密、不解析、不持久化、不记录 `ciphertext`。

## 7. 业务多路复用

Noise 通道内使用统一 envelope：

```text
rpc.request  {id, method, params}
rpc.response {id, result}
rpc.error    {id, code, message, retryable}
event        {topic, data}
ping / pong
stream.open  {streamId, service, metadata}
stream.data  {streamId, data(base64url)}
stream.close {streamId, code, reason}
stream.error {streamId, code, message}
```

- `streamId` 由发起方分配，按连接隔离；同一连接内唯一。
- Host 只接受 allowlist 内的方法/服务；未知 method、越权会话、错误 target fail closed。
- 首期业务适配器：
  - `remote.echo` / `remote.ping` / `remote.host.info`；
  - `remote.http.request`：经 E2EE 调用 Host 本机 `/api/*`，Host 注入本地令牌；
    `/api/lan/*` 与 `/api/remote-agent/*` 拒绝；
  - `remote.ws` stream：桥接 Host 本机 `/ws*`，可选握手头（如
    `x-mode-rpc-host-capability`），禁止覆盖 Cookie/Authorization。
- 后续把 Harness RPC、文件、终端、远程桌面信令/媒体接入同一通道。适配器不得绕过
  Host 本地权限。

## 8. 账号服务 API

所有路径在新服务命名空间下，无 v2 兼容：

```text
POST   /api/v1/auth/github                  {accessToken} -> web session
POST   /api/v1/auth/refresh                 web session 轮换
POST   /api/v1/auth/logout
GET    /api/v1/auth/me

POST   /api/v1/devices                      {device descriptor} -> access+refresh
POST   /api/v1/devices/register-owned-role  {device descriptor} -> 用设备凭证注册相反角色
POST   /api/v1/devices/refresh              {deviceId, refreshToken} -> 轮换
GET    /api/v1/devices                      同账号异角色设备目录（无密钥/无私密）
GET    /api/v1/devices/{id}                 membership 保护的 descriptor
DELETE /api/v1/devices/{id}                 撤销设备（owner 或自身）

GET    /api/v1/memberships                  当前设备的有效关系
DELETE /api/v1/memberships/{peerDeviceId}   撤销该 pair

POST   /api/v1/connections                  创建连接授权，返回 connectionId
GET    /api/v1/connections/{id}             连接元数据（双方设备）
DELETE /api/v1/connections/{id}             主动关闭
GET    /api/v1/ice                          （预留 WebRTC）
```

- `GET /api/v1/devices` 只返回 descriptor + 在线状态 + 地址，绝不返回任何 token。
- `POST /api/v1/connections` 只能由 Client 调用，返回 Host descriptor、
  `connectionId`、Relay 分配与 `directTicket`；Host 通过 Control 收到同一 connection。
- 所有设备查询从 token subject 的 membership 反查，防 IDOR。
- 错误 envelope：`{ "error": { "code", "message", "requestId", "retryable" } }`。

## 9. 限额与安全不变量

- REST body ≤ 64 KiB；配对/登录/刷新/注册独立限速；Control hello 5 秒超时。
- Control frame ≤ 64 KiB；Relay frame ≤ 1 MiB；每连接有界队列，慢消费者断开。
- 同一设备新 Control socket 替换旧 socket（`4003`）；撤销立即关闭相关连接。
- Server 不记录 token、私钥、Noise 明文、ciphertext、业务方法名/参数。
- Host 本地 API 只监听回环；LAN/公网不再直接暴露携带本机令牌的 HTTP 服务。
- 设备名称、在线状态、UI 选中状态都不是身份凭据；身份只来自固定公钥。

## 10. 配对 v1（跨账号/仅设备账号，后续实现）

保留协议位，首期不启用：

```text
Host 创建会话 -> 二维码/短 ticket -> Client 扫码确认 -> Server 生成 8 位验证码
-> Host 本机人工输入确认 -> 建立 device_only 账号与单条 membership
-> 双端获得设备凭证
```

- 服务端只存 ticket 的 SHA-256；会话 180 秒；确认一次性。
- 私钥持有证明：X25519(devicePrivate, serverPublic) → HKDF → HMAC 绑定请求方法/路径/body。
- Host 最终确认只能在本机 UI；模型工具/远程 Client 不得代提交。
- 新 Host `inbound_only=true`；新 Client 无 Web/Admin 权限。
- 不复用 v2 的 6 位 LAN 配对码。

## 11. 旧模块处置

| 旧路径 | 处置 |
| --- | --- |
| `packages/cloud-relay` 的目录令牌/`/d/<tunnelId>` 数据面 | v3 客户端不再使用；新服务上线后删除 |
| `packages/desktop` LAN 配对码 + 长期 Bearer | v3 远程控制不再使用；保留为纯 LAN 诊断/过渡，随后删除 |
| 远程桌面信令/媒体票据 `?token=` | 迁移到 Noise 通道后再删除 |
| `docs/specs/cloud-personal-accounts.md` v2 | 已标记退役；v2 `ICloudAccountService` 与账号 UI 已删除 |
| 旧 `MODE_CLOUD_*` 目录令牌配置 | 新服务不读取 |

**不做**双写、token 翻译、旧设备自动迁移；旧客户端升级后重新登录注册。

## 12. 状态与事件顺序（关键流）

```text
注册：
设备 -- POST /api/v1/devices (Web session, descriptor) --> 服务端
     服务端原子：upsert device + 建立/恢复 membership + 签发 token pair

连接：
Client -- connect.request --> Server
Server -- 校验 membership/在线 --> Host: connect.incoming
Host   -- GET /devices/{clientId} 交叉校验 + pin --> Server
Host   -- connect.accepted?（由 Server 下发客户端侧）--> Client
Client -- transport.selected --> Server --> Host
Client -- secure.handshake step1 --> Server --> Host
Host   -- secure.handshake step2 --> Server --> Client
双方   -- relay.attach(ticket) --> Relay --> relay.ready
       ===== Noise ciphertext =====

撤销：
Owner -- DELETE /devices/{id} --> 服务端撤 token/membership/连接
      --> Control 4004 / Relay close / 本地 pin 保留但不可再连
```

## 13. 验收

1. Noise IK：双方完成握手并双向收发；篡改、重放、乱序、错误 prologue、错误 static key
   全部失败；跨实现 golden vector 固定。
2. 分片：>48 KiB 消息往返正确；重复/乱序/超限/长度不一致失败。
3. 账号：同一 GitHub 账号重复登录复用账号；跨账号设备不可见；目录不返回任何 token；
   refresh 轮换与重放撤销；设备撤销后 REST/Control/Relay 全部拒绝。
4. 连接：非 membership、错误角色、Host 离线、跨账号连接均拒绝；Host 交叉校验失败拒绝；
   多 Client 可同时连接同一 Host；同 Client 新连接替换旧连接。
5. E2EE：中继捕获的字节中不出现业务明文、方法名或参数；Relay 不持久化密文。
6. 选路：LAN 可达时走 LAN；LAN 失败回落 Relay；握手完成后不切数据面。
7. 工程门禁：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、
   新增包测试全绿。

## 14. 实现状态（2026-11-11）

已落地：

- `packages/remote-core`：X25519 设备身份、Noise IK、Noise transport、DSHF 分片、
  Control/Secure/Business 帧严格编解码、`SecureSession`、`StreamMux`、`ControlLink`
  Host/Client 运行时（`packages/remote-core/src/app/controlLink.ts`）。
- `packages/remote-server`：SQLite 账号/设备/token/membership/connection schema、
  REST（GitHub 登录、设备注册/刷新/目录/撤销、membership、连接）、Control WS
  （hello/connect/handshake/relay/ping）、独立 Relay WS、签名 ticket、撤销断连。
- 测试：32 项（Noise 握手/篡改/重放/身份替换、分片、帧解析、账号隔离、目录不泄密、
  refresh 轮换与重放撤销、Control+Relay E2E、LAN direct 选路与回落、ControlLink 运行时、
  Desktop Host/工作区/隧道、v3 direct WS 入口与账号展示）。
- 门禁：`pnpm typecheck`、`pnpm lint`（0 error）、`pnpm architecture:check --changed`
  全绿。

P3 已接线的部分：

- `packages/desktop/src/host/remoteV3/`：Host 设备身份持久化、GitHub Device Flow、
  设备注册、Control 常驻、peer pinning、RPC allowlist（`remote.ping` / `remote.echo` /
  `remote.host.info` / `remote.workspace.list`）。
- 同一安装可用 Host 设备凭证通过 `register-owned-role` 注册独立 Client 角色，
  `IRemoteV3Service.request` 直接走 E2EE 通道执行一次 RPC。
- 服务以 local-only 频道 `remote-v3` 注册进 Host；初始化失败只记录日志，不阻断 Host 启动。
- 账号登录 UI 已重做：`ModeAccountDialog` 改为 `RemoteV3AccountSection`（服务地址输入、
  GitHub Device Flow 验证码/授权链接、在线状态、设备指纹、活动连接、只读设备目录、
  退出远程控制）；v2 的 `RemoteControlCloudSection` 已删除。
- 通用 E2EE WebSocket 隧道：Host 侧把 `remote.ws` stream 桥接到本机 LAN 的 `/ws*`
  （token 只进本机 Cookie，帧首字节区分 text/binary，可选握手头）；Client 侧通过
  `openTunnel`/`sendTunnel`/`closeTunnel` 与 `onTunnelEvent` 复用。隧道要求本机 LAN
  服务已启用，否则返回 `local api unavailable`。
- 隧道事件在无订阅者时先缓存（上限 256），首个 `onTunnelEvent` 订阅者会收到补齐的
  `open`/首帧；避免渲染进程「先 openTunnel、再订阅」之间丢掉 `Initialize`。
- Host 注册时上报本机 LAN 地址（最多 8 个 `host:port`，不含令牌），设备目录按设备返回；
  远程控制页优先展示该地址。直连数据面仍未启用，地址目前只作为 P2 选路输入。
- `remote.workspace.list` 已接线到 `collectLanAccessWorkspaces`（工作区列表来自设置），
  不再返回 `METHOD_NOT_FOUND`。
- 工作区隧道级重连：`attachV3RemoteWorkspace` 用稳定 ISocket 包住可重开的 `/ws/host`
  隧道；断开时重置 `SocketProtocol` 半包、拒绝在途 RPC，重连后新 ChannelServer 的
  Initialize 触发 `ChannelClient` 重放事件订阅，services 引用保持不变。重连失败 10 次
  才把 workspace 标记为断开。
- Host HTTP 桥：`remote.http.request` 经 E2EE 调用本机 `/api/*`；Host 注入本地令牌，
  只放行 `content-type`/`accept`/`x-mode-rpc-host-capability`，拒绝 `/api/lan/*` 与
  `/api/remote-agent/*`；请求/响应体各限 2 MiB。Client 侧为 `httpRequest`。
- 远程桌面 v3 主控：`listHosts` 读取同账号 Host；`startRemoteDesktop` 经 HTTP 桥创建
  双票据会话，再把信令/媒体 `remote.ws` 隧道桥接到本机 `127.0.0.1` 回环代理
  （`RemoteDesktopTunnelProxy`），现有 controller 与 renderer 媒体客户端无需改协议。
  媒体隧道断开时 signal 会话与回环 URL 保留；renderer 带 `mediaTicket` 重连后，
  代理按新票据重开 `remote.ws` 媒体流，实现媒体隧道级重连。
- 远程控制页把 v3 Host 投影为 `origin:"v3"` 设备行，`rdStartSession(hostDeviceId)` 由
  Host 的 `openV3Session` 路由到上述桥；非 v3 设备继续走原有 LAN 链路。
- 远程工作区 v3：`RemoteTarget` 新增 `{ kind: "v3", deviceId }`，identity 固定为
  `remote:v3:<deviceId>:<path>`、environment key 为 `v3:<deviceId>`；Host 侧
  `attachV3RemoteWorkspace` 先经 HTTP 桥读取 `/api/server-info` 与
  `/api/rpc-host-capability`，再打开带 capability 头的 `/ws/host` E2EE 隧道，包装成
  RPC `ISocket` 后复用 LAN 的 `SocketProtocol → ChannelClient`。连接向导新增 v3 设备页
  （只列在线 Host），历史记录快照只持久化 deviceId/展示名。
- 测试：v3 工作区挂接夹具走真实 HTTP/WS 服务端，覆盖 capability 票据、隧道帧格式、
  `Initialize` 时序与首条 RPC。
- Control 链路自愈：Host 观察到非主动关闭后按 full-jitter 指数退避（500ms 起、
  30s 封顶）自动重连；401/403 凭证失效不重试并保留错误态。Client 侧断链会立即失败
  同设备在途 RPC，避免等 15s 超时。心跳增加 pong 看门狗：连续一个周期没有 pong
  即主动关闭，触发上述重连。
- 账号展示：`/api/v1/auth/me` 的 `account.displayName` 落库并进入 Host 状态；左下角
  账号区与账号弹窗优先使用 v3 展示名，`reconnecting` 仍算已登录；旧云账号名只作兜底。
  侧边栏退出菜单在 v3 已登录时调用 `disconnectAccount`。
- 账号切换：配置时对比 `/api/v1/auth/me` 与本地记录的 accountId；账号变化或注册
  返回 409（`device_conflict` / `device_revoked`）时轮换 Host/Client 设备身份并清除旧 token 与 peer pin，
  避免“deviceId 已属于其他账号”。同账号重启不会轮换，peer pin 保持有效。
- LAN direct：`connect.incoming` / `connect.accepted` 携带同一 `direct {ticket,addresses}`；
  Client 先连 `ws://<address>/ws/v1/direct`，首帧 `direct.attach` 由 Host 的 pending
  ticket 校验，成功后 `relay.ready`，失败回落 Relay。Token 数据面帧仍是 Noise 密文。
  测试覆盖直连成功、HTTP 入口 ticket 拒绝与 Relay 既有路径。
- 刷新单飞：refresh token 单次使用，Host/Client 的刷新请求共享同一个在途 Promise，
  并用 generation 丢弃账号切换后才完成的旧刷新结果，避免并发刷新被判定为重放而
  撤销整个 token family。可恢复断链对外状态为 `reconnecting`，凭证失效才进入 `error`。

仍在进行：

- 媒体与工作区隧道均已支持重开；Control 中断后的业务 session lease 恢复仍未实现。
- Web 客户端：`@mode/remote-core/web` 已提供 Noise/X25519、`WebControlLink`
  （Control + Relay + 业务帧 + 自动重连）和 `WebAccountClient`
  （身份/注册/刷新/目录/GitHub Device Flow 代理）；`/v3` 页面已提供登录、设备目录、
  加密 RPC 与工作区隧道连通测试；完整工作区/远程桌面 Web 页面仍待实现，见
  [remote-control-v3-web.md](./remote-control-v3-web.md)。
- WebRTC/P2P/TURN、Control 中断 lease 恢复；LAN direct 已落地但 LAN 监听本身仍是 WS（无 TLS）。
- 跨账号扫码配对 v1（§10）。
- v2 旧模块删除：v2 `ICloudAccountService` 与账号 UI 已删除；`packages/cloud-relay`
  目录令牌/`/d` 数据面、LAN Bearer、远控票据仍需 LAN 路径替代后删除。
- 生产 TLS 已支持进程内终止（`tls.key/cert` 或 `MODE_REMOTE_TLS_*_FILE`，URL/Relay 自动
  切到 https/wss）；反向代理仍可用于统一入口。`/metrics` 已暴露 Control 连接数、逻辑
  连接数、Relay 帧/字节与 API 请求计数（需 `MODE_REMOTE_METRICS_TOKEN`），`/ready`
  提供 DB 探针；SQLite schema 使用 `PRAGMA user_version` 做幂等迁移。全局限流目前仍只有
  固定窗口，未接 tracing。
- v3 远程桌面要求被控 Host 的本机 LAN 服务已启用；未启用时 HTTP 桥返回
  `HOST_LOCAL_API_UNAVAILABLE`。
