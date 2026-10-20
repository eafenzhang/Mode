# Spec：服务端形态的局域网被连端（lanAccessHost）

状态：已实现（2026-10-13；真机验收见文末）。
关联：[lan-paired-devices.md](./lan-paired-devices.md)（配对设备管理 UI 与客户端侧元数据）、[lan-remote-attach.md](./lan-remote-attach.md)（挂接握手时序）、[fnos-fpk-packaging.md](./fnos-fpk-packaging.md)（NAS 形态部署）。

## 背景与目标

- 「设置 → 局域网访问 → 允许局域网访问」此前只有桌面端实现（桌面 Host 内的 lanAccessServer：
  监听、发现应答、配对、逐设备令牌）。Web 与 NAS（fpk 的 packages/server）形态是显式 stub
  （`packages/web/src/main.tsx` 四个 reject，报错原文 "Web mode does not serve LAN access"）。
- 目标：让**服务端形态（packages/server 的 entry-http，覆盖 fpk 与 web）也能作为「被连端」**：
  同一网段/可达网络内的其它 Mode 桌面端用一次性配对码换取逐设备长期令牌，经既有
  `/api/server-info` → `/api/rpc-host-capability` → `/ws/host` 通道挂接并使用本机工作区。
- 复用优先：配对端点、逐设备令牌校验、workspacePath 录制回调全部复用 packages/server 既有
  hooks；`shared/lanAccess.ts` 明示「服务端实现、packages/server 消费的配对端点契约」。

## 产品规则

1. **开关**（设置 → 局域网访问 → 允许局域网访问）：
   - 持久化于凭据服务键 `lan:access:enabled`（服务端本地键，不随 shared 契约外发），默认关闭，
     升级/重启保持。
   - 关闭 = 不对外服务：配对码作废、逐设备长期令牌一律校验失败；**静态访问令牌不受影响**
     （浏览器/网关/直连链路照常）。
   - 开启 = 允许配对与已配对设备挂接；状态回填 enabled / port / addresses。
2. **配对码**：6 位大写十六进制（3 随机字节），TTL 5 分钟（`LAN_ACCESS_PAIR_CODE_TTL_MS`），
   单次有效；重新生成即替换；兑换先作废再落盘（防重放）；「码错/过期」不区分（防探测）。
   开关关闭时不签发新配对码。
3. **逐设备长期令牌**：32 随机字节 base64url，配对时一次性下发；落凭据服务
   （`lan:access:client:<id>:token`，键由 shared 构造器生成）。记录
   （id/label/createdAt/lastUsedAt/lastWorkspacePath）落 `lan:access:clients`（JSON 数组，
   经 shared `lanAccessClientRecordSchema` 口径解析）。
   - label = 配对方主机名（挂接侧传入，缺省 "LAN client"）。
   - lastUsedAt：命中令牌校验时更新，每分钟至多落盘一次（与桌面端同口径，避免凭据写热点）。
   - lastWorkspacePath：`/ws/host` 连接上携带 `workspacePath` 的 RPC 触发录制（既有
     `onLanClientWorkspace` 通道），同路径重复上报忽略。
4. **对端列表**：设置页展示 label / 最近使用 / 最近打开目录；逐个移除（删令牌+记录）、
   整体重置（清空全部令牌与记录）。
5. **serverId**：安装级稳定标识（凭据键 `lan:access:server-id`，缺失时生成并落盘），与桌面端
   同键同语义；server-info 的 serverId 改由它提供（此前回落到主机名），挂接侧「我配对的对端」
   按 serverId 归档才稳定。
6. **复用既有端点（协议零改动）**：
   - `POST /api/lan/pair`（免令牌自证；body `{code,label}` → `{token,serverId,name}`）
   - `GET /api/server-info?token=`（挂接侧读设备名与工作区）
   - `POST /api/rpc-host-capability`（cookie 携长期令牌换一次性 capability）
   - `ws /ws/host`（cookie + `MODE_RPC_HOST_CAPABILITY_HEADER`）

## 新端点（均要求访问令牌：/api/* 既有中间件覆盖）

| 方法 | 路径 | 语义 |
| --- | --- | --- |
| GET | `/api/lan/state` | 返回 `LanAccessState`（enabled/port/addresses/pairCode/clients） |
| POST | `/api/lan/enable` | body `{enabled:boolean}`（strict）→ 落盘并返回状态 |
| POST | `/api/lan/pair-code` | 生成配对码并返回状态；关闭态返回状态但不生成 |
| DELETE | `/api/lan/clients/:clientId` | 移除单个已配对设备 → 状态 |
| POST | `/api/lan/reset` | 重置全部令牌与记录 → 状态 |

响应一律为 shared `lanAccessStateSchema` 形状；非法 body → 400。

## 状态所有权与事件顺序

- 卡片状态唯一所有者 = packages/server 的 `lanAccessHost`（内存缓存 + 凭据服务持久化，
  写入者仅它一家）；http.ts 只做端点转发，web 平台只做 fetch。
- 挂接时序（沿用 lan-remote-attach，不改）：
  配对换令牌 → `GET /api/server-info?token=` → `POST /api/rpc-host-capability`（cookie）→
  `ws /ws/host`（cookie + capability 头）→ RPC；录制回调把 workspacePath 写回该 clientId 记录。
- 鉴权组合：`verifyAuthToken = 静态令牌 === presented || lanAccessHost.verifyToken(presented)`；
  `resolveLanClientByToken` 只为 lan 令牌返回 clientId（静态令牌连接不参与录制）。

## 失败语义

- 关闭开关：`redeem` 返回 null（`POST /api/lan/pair` → 401）；lan 令牌校验失败 → 后续 401。
- 无访问令牌配置（本地 dev 无 token）：**不装配** lanAccessHost，不加鉴权中间件，行为与现状一致。
- 凭据写失败：仅告警不回滚内存态（与桌面端实现同口径）。
- 挂接侧「码错/过期/已被重置」文案由客户端按 401 统一处理（协议既有语义）。

## 非目标

- UDP 发现应答（桌面端已有；服务端形态先用「手动输入地址」挂接），列为后续。
- 客户端侧「我配对的对端」（浏览器去连别的设备）仍为 Web stub。
- 远程桌面 / 电脑控制能力位不声明（服务端形态非 win32 被控桌面）。
- 配对尝试速率限制：已在 `packages/server/src/http.ts` 统一实现（30 次/分钟，覆盖
  `/api/lan/pair` 与 `/api/remote-agent/enroll`），桌面与 NAS 形态同口径
  （见 [lan-remote-public-endpoints.md](./lan-remote-public-endpoints.md) 安全边界）。

## 验收场景

1. （server 包测试）开启 → 生成码 → `POST /api/lan/pair` 换令牌 → `server-info`（200、serverId =
   持久 serverId）→ capability → `/ws/host` 挂接 → 携带 workspacePath 的 RPC 触发录制、
   lastUsedAt 更新；静态令牌访问 `/api/lan/state` 全程 200（web UI 回归）。
2. 关闭态：pair 401；lan 令牌 server-info 401；静态令牌仍可用。
3. 配对码一次性：同一码二次兑换 401；错误码不影响有效码继续可用。
4. 移除单个客户端 → 其令牌 401 且列表少一行；重置 → 全部 401、列表空。
5. `POST /api/lan/enable` 非法 body → 400。
6. 门禁：typecheck / lint / server 测试 / 架构 / 语言政策全绿。
7. 真机（NAS，待执行）：设置页开关可开、生成配对码、桌面端手动地址 + 配对码挂接成功。

## 真机验收（2026-10-13 记录位）

已通过（fpk 0.0.22 装于飞牛 NAS，客户端为本机经 Tailscale + 真实 `pairLanPeer` /
`attachLanRemoteConnection` 代码）：

1. 设置页开关可开，状态行回读 `正在监听 192.168.31.76:3030、172.17.0.1:3030、100.104.219.122:3030`。
2. 配对码生成（UI 与静态令牌 curl 两条路径）→ 真实客户端配对成功：serverId 为持久安装标识、
   name=MiniNas、令牌 43 字符；配对码一次性消费（事后 state `pairCode:null`）。
3. 配对 → `GET /api/server-info?token=` → `POST /api/rpc-host-capability`（cookie）→
   `ws /ws/host` 挂接 → 首条 RPC `systemService.info()` 真实往返（platform=linux、
   homedir=/vol1/@apphome/mode）。
4. 设置页「配对过本机的设备」列表显示 `win-dev-box` 与最近使用时间（lastUsedAt 由令牌校验更新）。
5. 注意口径：配对码 TTL 5 分钟，人工取码 → 挂接的链路要在窗口期内完成；超时报「码无效或已过期」属预期。
