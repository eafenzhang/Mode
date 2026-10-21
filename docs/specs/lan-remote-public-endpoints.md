# 公网远程桌面端点（内置中继 + 公网优先 + LAN 回退）

状态：2026-10-21 实现中；同日扩展到远程工作区（`/ws/host` 与 HTTPS/WSS 端点）。
关联：[remote-control-device-model.md](./remote-control-device-model.md)（统一设备/端点模型）、
[in-app-remote-desktop.md](./in-app-remote-desktop.md)（远控产品规则）、
[lan-remote-desktop-native-media.md](./lan-remote-desktop-native-media.md)（媒体面）、
[lan-remote-attach.md](./lan-remote-attach.md)（挂接握手）、
[remote-control-unified-pairing.md](./remote-control-unified-pairing.md)（统一配对码与设备列表）。

## 背景与目标

现役远控只在 LAN/Tailscale 直连下工作：对端地址被写成 `http://host:port`，信令与媒体
WebSocket 固定 `ws://`，且一个已配对设备只保存一个地址。用户已有域名与 fnOS，能通过
反向隧道把 Windows 被控端的 45880 暴露到公网，但客户端没有 HTTPS/WSS 与多端点能力，
因此公网链路无法使用。

本 spec 解决**内置公网中继的公网端点、自动下发与连接顺序**：仓库已新增
`packages/cloud-relay`（WSS 中继 + 出站隧道，见
[wss-relay-mvp.md](./wss-relay-mvp.md)）。被控端配置中继令牌后自动启动出站隧道，
并把 `https://relay.mymode.cloud/d/<tunnelId>` 随配对响应与 `server-info` 下发给主控。
仍不实现 NAT 穿透/打洞；用户自建的 frp/Caddy 入口仍可作为显式公网地址覆盖。

```text
主控 ── https/wss://relay.mymode.cloud/d/<id> ──► Cloudflare Tunnel ──► fnOS 中继
                                                                          │
                                                                          ▼
                                                                   被控端出站隧道
                                                                          │
                                                                          ▼
                                                                   127.0.0.1:45880

局域网主控 ── http/ws://192.168.x.x:45880 ──► 被控端（公网不可达时回退）
```

## 产品规则

1. **一个已配对设备可有多个端点**：LAN 端点（`http`）与公网端点（`https`）并存；
   每个端点包含 `scheme` / `host` / `port` / `kind`。旧记录（只有 `host` + `port`）
   在读取时按 LAN `http` 端点迁移，不要求用户重新配对。
   公网端点默认由被控端随配对响应/`server-info` 的 `publicRelay` 自动下发
   （内置 `DEFAULT_PUBLIC_RELAY_BASE_URL` + 持久化 `tunnelId`），主控不需要手填；
   显式输入的 `publicBaseUrl` 仍可覆盖。
2. **公网端点必须是 HTTPS**：`kind = "public"` 只接受 `https`，并据此使用 `wss` 建
   信令/媒体 WebSocket。拒绝 `http` 公网端点，**不允许静默降级到明文**。
3. **公网优先、LAN 回退**：连接按公网 → LAN 的顺序尝试；有多个端点时公网探测预算
   8s，LAN 回退预算 2.5s；只有一个端点时沿用既有 10s。选择顺序由持久化的
   `endpoints` 数组决定，公网端点始终排在 LAN 端点之前。
4. **对端级错误不重试另一个端点**：探测阶段只有传输失败/超时/5xx/404/协议不符才
   切换端点；401/403 与合法 `server-info` 立即收口（同一令牌在另一个端点也不会更好）。
   会话阶段先探测选出可达端点，再向该端点 POST；POST 返回的 `busy`、`consent_denied`、
   `disabled`、`unauthorized`、`unsupported` 等都是对端级结果，**不得再向另一个端点
   重发**，避免重复弹同意窗或重复占用会话。
5. **配对**：
   - LAN 配对路径不变（发现/手动地址 + 6 位码）。
   - 被控端配置了内置中继时，配对响应带 `publicRelay`；主控自动写入
     `https://<baseUrl>/d/<tunnelId>` 作为公网端点。显式输入的 `publicBaseUrl` 优先。
   - 已配对设备可在设备行新增/替换/清除公网地址；只改本机凭据里的端点，不动令牌。
   - 远程控制页提供“公网地址 + 配对码”入口，允许在无法直连 LAN 时直接用 HTTPS
     地址完成首次配对；成功后该设备以公网端点进入统一设备列表。
   - 重复配对同一 `serverId` 时合并端点：本次配对的端点优先作为主地址；已有 LAN
     端点保留在后；新配对带公网端点时替换旧公网端点，不带时保留已配置的公网端点。
     令牌以本次配对下发的为准。
6. **展示**：设备行显示主地址与公网地址；探测结果附带本次命中的端点类型
   （`lan` / `public`），状态胶囊与“进入桌面”可用性判定保持既有口径（在线 + 能力位 +
   原生媒体面版本门）。
7. **局域网能力保留**：UDP 发现、配对码、LAN `http/ws` 直连、Tailscale 单播探测全部
   不变；公网端点只是附加路径。
8. **远程工作区同源**：远程工作区的 `RemoteTarget` 携带同一个 `endpoints` 数组；
   `server-info`、`/api/rpc-host-capability`、`/ws/host` 全部按选中端点的
   `scheme`/`path` 拼接，`https → wss`。远程工作区与远程桌面共享端点排序、探测预算
   （公网 8s、LAN 2.5s）、401/403 立即收口与“会话阶段不换端点”的语义。仅公网端点的
   设备必须能连接工作区，不再被向导过滤。

## 状态所有者

| 状态 | 唯一所有者 | 说明 |
| --- | --- | --- |
| 对端令牌 + 端点元数据 | Host 凭据服务（键 `lan:peer:<serverId>:token` / `:meta`） | 写入只经 `pairLanPeer` 与 `setPairedPeerPublicEndpoint` |
| 端点编辑命令 | `lan-access` Host 服务（`ILanAccessService`） | renderer 只发命令并重读列表 |
| 中继 tunnelId | Host 凭据服务（`lan:access:relay-tunnel-id`） | 首次启用中继时生成并持久化；配对/`server-info` 只读 |
| 中继隧道生命周期 | `LanAccessServerHandle` | 启用 LAN 且配置中继令牌时启动，dispose 时关闭 |
| 探测/会话端点选择 | `remoteDesktopPeerClient` | 只读凭据；不缓存、不持久化选择结果 |
| 设备页探测状态 | `RemoteDesktopHome` 页级瞬态 | 与既有能力位探测同一所有者 |

`remoteDesktopPeerClient` 不保存“上次成功的端点”；每次探测/会话重新按顺序（公网 →
LAN）尝试，避免网络切换后使用陈旧路由。写入端点的两个入口都做 HTTPS 校验，`kind`
不允许由调用方伪造为 `public` + `http`。

## 接口

### 共享 schema（`packages/shared/src/lanAccess.ts`）

```ts
type LanPeerEndpointKind = "lan" | "public";
interface LanPeerEndpoint {
  scheme: "http" | "https";
  host: string;
  port: number;
  kind: LanPeerEndpointKind;
}
interface LanPeerMeta {
  endpoints: LanPeerEndpoint[];
  name?: string;
  pairedAt?: number;
}
interface LanPublicRelayInfo {
  baseUrl: string;
  tunnelId: string;
}
```

- `DEFAULT_PUBLIC_RELAY_BASE_URL = "https://relay.mymode.cloud"`；可在 Host 侧用
  `MODE_PUBLIC_RELAY_BASE_URL` 覆盖。
- `LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY` 保存被控端持久化的 `tunnelId`。
- `LanAccessPairResult.publicRelay?: LanPublicRelayInfo`：配对响应自动下发内置中继。
- `ServerRemoteInfo.publicRelay?: LanPublicRelayInfo`：`/api/server-info` 同样下发，
  供已配对主控补全/刷新。
- `LanAccessState.publicRelayUrl?: string | null`：设置页展示当前公网入口。
- `lanPeerMetaSchema` 同时接受 v2（`endpoints`）与 v1（`host`/`port`）；v1 在
  `parseLanPeerMeta` 中迁移为单个 LAN 端点。
- `LanPairPeerRequest` 增加可选 `scheme?: "http" | "https"` 与
  `publicBaseUrl?: string`；`publicBaseUrl` 支持 `rd.example.com[:port]` 简写，
  统一按 HTTPS 解析，缺省端口 443。
- `LanPairedPeer` 增加 `endpoints?: LanPeerEndpoint[]`；`host`/`port` 保留为主端点
  （公网优先），兼容既有 UI/测试。
- 公网 URL 解析器只接受无 userinfo、无 query/hash 的 HTTPS 地址；允许中继路径前缀，
  端口缺省 443。

### Host 服务

- `pairLanPeer({ host, port, code, scheme?, publicBaseUrl? })`：按主端点 scheme 发
  `POST /api/lan/pair`，成功后把主端点与公网端点一起写入 meta；公网端点来自显式
  `publicBaseUrl`，否则取响应里的 `publicRelay` 自动生成
  `https://<baseUrl>/d/<tunnelId>`。
- `startLanAccessServer({ relay?, startTunnel? })`：`relay.token` 非空时生成/复用
  `tunnelId`，启动 `startRelayTunnelClient`（`wss://<baseUrl>/_tunnel` →
  `http://127.0.0.1:<port>`），并在配对响应与 `server-info` 中带 `publicRelay`；
  未配置令牌时保持 LAN-only。
- `ILanAccessService.setPairedPeerPublicEndpoint({ serverId, publicBaseUrl })`：
  `publicBaseUrl = null` 清除公网端点；清除后若没有其它端点则拒绝（提示直接删除配对）。
  新增时替换已有公网端点，保留 LAN 端点、名称与令牌。
- `probeRemoteDesktopPeer` / `openRemoteDesktopPeerSession` / `getLanPairedPeerWorkspaces`
  统一按端点数组尝试；会话 URL 的 WebSocket 协议由端点 scheme 推导：
  `https → wss`，`http → ws`。
- `RemoteDesktopPeerCapability` 增加可选 `endpointKind` / `endpointHost`，只用于展示
  本次命中的路由，不参与可用性判定。

### UI

- `RemoteControlPublicPairing`：HTTPS 地址 + 配对码，调用 `pairLanPeer({ scheme: "https" })`。
- `RemoteControlLanPeerRow`：新增“公网地址”编辑入口；保存/清除后由设备页重读列表。
- 设备行地址显示主端点（有公网端点时即公网），并在存在 LAN 端点时追加 LAN 地址；
  探测命中公网时详情文案标注“公网”。

## 事件顺序

```text
① 配对（LAN）
主控 ── POST http://<lan>:45880/api/lan/pair ──► 被控
     ◄── {token, serverId, publicRelay?} ── 写 meta{endpoints:[公网(自动), LAN]}

② 配对（公网）
主控 ── POST https://<domain>/api/lan/pair ──► 隧道 ──► 被控
     ◄── {token, serverId, publicRelay?} ── 写 meta{endpoints:[公网]}

③ 探测
probeRemoteDesktopPeer → meta.endpoints
  PUBLIC 端点（若有）── GET https://.../api/server-info（8s）──►
    成功：返回 online + endpointKind=public
    可重试失败：下一个端点
  LAN 端点 ── GET /api/server-info（2.5s）──►
    成功：返回 online + endpointKind=lan
    失败：online:false

④ 会话
openRemoteDesktopPeerSession → 先探测选端点
  ── POST <endpoint>/api/remote-desktop/session（90s，含同意窗）──►
    成功：wsUrl/mediaUrl 用同一个端点的 scheme（https→wss）
    对端级错误：原样返回稳定错误码，不再换端点

④' 远程工作区
RemoteTarget.endpoints → 按同一顺序探测 /api/server-info
  ── POST <endpoint>/api/rpc-host-capability（Cookie 令牌）──►
  ── WS <endpoint>/ws/host（scheme https→wss，path 保留 /d/<tunnelId>）──►
    成功后该连接绑定选中的端点；会话期间不换端点；断开即 logical session closed

⑤ 编辑公网地址
UI ── setPairedPeerPublicEndpoint ──► Host 读 meta → 校验 HTTPS → 替换/清除 ──► 保存
   ── 重读 listPairedPeers → 重新探测
```

## 失败语义

- 公网地址不是 HTTPS、带 query/userinfo、端口非法 → 命令拒绝，UI 显示可读错误。
- 证书不受信任/主机名不匹配 → 传输失败；**不回退到 http**，设备显示离线或内部错误。
- 公网端点不可达但 LAN 可达 → 回退成功，设备在线；会话只向 LAN 端点请求。
- 公网端点返回 401/403 → 直接收口（同一令牌），不尝试 LAN。
- 被控端未配置中继令牌 → 配对响应无 `publicRelay`，主控保持 LAN-only（旧版行为）。
- 会话 POST 返回 `busy` / `consent_denied` / `consent_timeout` / `disabled` /
  `unauthorized` / `unsupported` / `locally_controlled` → 原样返回，不重发。
- 所有端点都不可达 → 探测 `online:false`；会话返回 `internal`（保留最后一条传输错误
  的可读文案）。
- 旧 meta（只有 `host`/`port`）→ 按 LAN `http` 端点读取；重命名/删除/工作区读取行为不变。

## 安全边界

- 公网端点强制 TLS；令牌仍走 query/cookie，禁止在公网用明文 HTTP。
- 内置中继（fnOS + Cloudflare Tunnel）在 Cloudflare 边缘终止 TLS，relay 能看到明文
  令牌、键鼠事件与剪贴板上行。这是可信自建中继的边界；开放给第三方前必须先做上行 E2E。
- 中继令牌是部署 secret：只从 env/启动参数读取，不写入仓库；中继与反代日志不得记录
  完整 query（令牌、会话票据、mediaTicket 都在 query 里）。
- 下行媒体仍是逐帧 AES-256-GCM；`wss` 只保护传输层，不替代媒体加密。
- 6 位配对码在公网暴露后必须限速：`/api/lan/pair` 与 `/api/remote-agent/enroll` 共用
  同一个码，server 实例内统一限 30 次/分钟；5 分钟窗口最多 150 次尝试，远小于
  6 位十六进制码空间（约 16.7M）。
- 日志不写令牌、`mediaTicket`、配对码或完整 URL query；只记录端点 kind/host 与错误类别。

## 验收场景

1. 旧记录迁移：凭据里只有 `{host,port,name}` 时，设备列表仍显示、可探测、可进入桌面。
2. LAN 配对后新增 `https://rd.example.com`：设备行显示两个端点；公网可达时连接命中
   `public`；公网断开/不可达后回退 `lan`，画面、输入、剪贴板正常。
3. 公网地址配对：无 LAN 直连时输入 HTTPS 地址 + 6 位码，配对成功并出现在设备列表。
4. 公网地址输成 `http://`、带 path 或端口非法 → 拒绝并提示；`https` 自签证书失败时
   不降级到 http。
5. 会话端点级错误（如 `busy`）只出现一次请求，不向另一端点重发（抓包/日志断言）。
6. 清除公网地址后回到仅 LAN；只有公网端点的设备拒绝清除并提示删除配对。
7. LAN 发现、配对码、Tailscale 直连、既有 LAN 远程工作区挂接不回归。
7'. 远程工作区：同一设备公网可达时 `/api/server-info`、`/api/rpc-host-capability` 与
   `/ws/host` 全部命中公网端点并使用 `wss`；公网断开后回退 LAN；仅公网端点的设备
   可以完成目录选择与工作区打开，不再被向导过滤。
8. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 与相关 Node 测试全绿。
9. 内置中继：被控端配置中继令牌后，配对响应带 `publicRelay`；主控自动出现公网端点且
   排在 LAN 之前，用户不需要手填地址。
10. 公网优先：两个端点都可达时探测只命中公网；公网不可达时回退 LAN，会话 URL 的
    `wss`/`ws` 随端点协议切换。
11. 未配置中继令牌：配对响应无 `publicRelay`，行为与旧版 LAN-only 一致。

## 部署示例（fnOS + frp + Windows Caddy，TLS 终止在被控端）

以下只是参考拓扑，不是代码依赖；把 TLS 放在 Windows 被控端，fnOS 只看得到密文。

1. **DNS**：`rd.example.com` 的 A/AAAA 指向 fnOS 公网 IP（动态 IP 用 DDNS）。
2. **fnOS** `frps.toml`：
   ```toml
   bindPort = 7000
   auth.method = "token"
   auth.token = "<长随机串>"
   ```
   若 fnOS 的 443 已被网关占用，把下面的 `remotePort` 改成 8443，客户端地址相应写成
   `https://rd.example.com:8443`。
3. **Windows 被控端** `frpc.toml`：
   ```toml
   serverAddr = "rd.example.com"
   serverPort = 7000
   auth.method = "token"
   auth.token = "<同一个长随机串>"

   [[proxies]]
   name = "rd-tls"
   type = "tcp"
   localIP = "127.0.0.1"
   localPort = 8443
   remotePort = 443
   ```
4. **Windows 被控端** `Caddyfile`（证书用 DNS-01 签发，80 端口不必暴露）：
   ```caddyfile
   rd.example.com:8443 {
     tls C:\certs\rd.example.com\fullchain.pem C:\certs\rd.example.com\privkey.pem
     reverse_proxy 127.0.0.1:45880
   }
   ```
   Caddy 自动处理 WebSocket upgrade；证书续期后 reload。
5. **防火墙**：fnOS 只开 443/7000；Windows 的 8443 只允许回环，45880 只允许局域网；
   Windows 不需要入站 443。
6. **配对**：主控在远程控制页的「公网设备」填入 `https://rd.example.com` + 被控端生成的
   6 位码；或在 LAN 内先配对，再用设备行的「公网地址」补上域名。之后探测与会话按
   公网 → LAN 自动选择。内置中继的部署见 [wss-relay-mvp.md](./wss-relay-mvp.md)。

注意：推荐 Let's Encrypt 等公共 CA；自签证书需要给 Host sidecar 配置
`NODE_EXTRA_CA_CERTS`，并让 Windows/Tauri 信任根证书，否则 TLS 握手会失败（且不会
回退到 http）。fnOS 若在 CGNAT 后面，域名无法直接解析到它，需要一台有公网 IP 的
中转或 Tailscale/WireGuard 之类的叠加网。反向代理/隧道**不要记录完整 query**的访问
日志：令牌、会话票据与 mediaTicket 都在 query 里，需要脱敏或关闭访问日志。配对完成后
若不再需要公网配对，可在反代上只放行 `/api/server-info`、`/api/remote-desktop/session`
与 `/ws/remote-desktop*`，关闭 `/api/lan/pair` 与 `/api/remote-agent/enroll`。

## 非目标

- 不实现 NAT 穿透/打洞、frp/Caddy 安装器；内置 relay 的部署见
  [wss-relay-mvp.md](./wss-relay-mvp.md)。
- 不做中继侧账号体系与逐设备令牌签发；当前中继令牌是部署级共享 secret。
- 不在公网端点上新增 Web/手机远控协议：浏览器远控复用 LAN 服务的静态资源与
  `web-remote-replayable`，公网访问仍经 HTTPS/WSS 反代或内置中继。
- 不做远程工作区的会话级端点切换：连接建立后断开即收口，由用户/重连重新选端点。
- 不在公网端点上做独立发现；由被控端在配对/`server-info` 中自动下发，显式地址可覆盖。
- 不做上行输入/剪贴板的端到端加密（TLS 终止在中继时的前提，属 P3）。
