# Mode Cloud 账号 + 设备目录（去链接配对）

状态：2026-10-21 MVP 已实现并部署（fnOS `/v1` 控制面 + SQLite）。关联：
[wss-relay-mvp.md](./wss-relay-mvp.md)（中继传输）、
[lan-remote-public-endpoints.md](./lan-remote-public-endpoints.md)（公网端点与配对）、
[lan-paired-devices.md](./lan-paired-devices.md)（配对记录）。

## 目标

主控登录 Mode Cloud 后，从账号设备目录看到自己的设备；点“配对”并输入被控端显示的
6 位码即可完成首次配对，**不再复制公网链接**。

```text
被控端 Desktop ── 本地微信 Bot 绑定 providerUserId
      │  Ed25519 设备密钥 + 部署 enrollment secret
      ▼
fnOS: cloud-relay 容器
  ├─ /_tunnel + /d/<tunnelId>      中继（不变）
  └─ /v1/devices, /v1/health       控制面 + SQLite 目录
      ▲
      │  Bearer deviceToken
主控 Desktop ── 设备目录（名称/在线状态/公网地址）
      │  选择设备 → 输入被控端 6 位码
      └─ pairLanPeer(https://relay/d/<tunnelId>, code)
```

## 产品规则

1. **默认无账号**：未绑定云账号时，LAN 发现、6 位码配对、手动公网地址全部照常工作。
2. **账号锚点 = 微信 Bot 绑定的用户 ID**：只接受启用且 `provider === "weixin"`、
   `providerUserId` 非空的 Bot。没有绑定时拒绝云绑定，并提示先绑定微信 Bot。
3. **云端不独立验证微信身份**：设备持有部署 `MODE_CLOUD_ENROLL_TOKEN`，用 Ed25519
   私钥签名上报微信 ID；云端信任该设备断言。这是本期明确接受的信任模型。
4. **目录只提供地址**：`GET /v1/devices` 返回设备名、平台、在线状态、公网地址；
   不返回 LAN 令牌、配对码或任何会话内容。
5. **配对仍由 6 位码授权**：主控从目录取 `relayUrl`，调用现有
   `POST /api/lan/pair`；云端不在令牌交换路径上。码错误/过期按现有 401 收口。
6. **在线状态**：以同进程中继的 tunnel 注册表为准；隧道断开即离线。目录不另建心跳。
7. **撤销**：撤销设备后目录不再返回；已建立的 LAN 配对记录不受影响。
8. **单账号单设备绑定**：同一 `serverId` 只能属于一个账号；换账号需要先撤销。
   同一设备重复 enroll 会更新元数据并签发新设备令牌。

## 状态所有者

| 状态 | 所有者 | 说明 |
| --- | --- | --- |
| 账号 / 设备 / 设备令牌哈希 | Mode Cloud SQLite（`cloud_*` 表） | 唯一持久化真相 |
| 中继 tunnel 注册表 | cloud-relay 进程内存 | 在线状态来源；重启后清空 |
| 设备私钥 / 账号 ID / 设备令牌 | Desktop 凭据服务 | 键 `cloud:device:private-key` / `cloud:account:id` / `cloud:device:token` |
| 云目录快照 | UI 页级瞬态 | 每次进入/操作后重读，不持久化 |
| 配对记录 | Host 凭据服务（现有 `lan:peer:*`） | 配对成功后复用现有设备列表 |

## 数据模型

```sql
cloud_accounts(
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,              -- 本期只允许 weixin
  provider_user_hash TEXT NOT NULL,    -- HMAC(pepper, "provider:providerUserId")
  display_name TEXT,
  created_at INTEGER NOT NULL,
  status TEXT NOT NULL,                -- active | disabled
  UNIQUE(provider, provider_user_hash)
)

cloud_devices(
  id TEXT PRIMARY KEY,                 -- 复用 install 级 serverId
  account_id TEXT NOT NULL,
  name TEXT NOT NULL,
  platform TEXT NOT NULL,
  version TEXT,
  public_key TEXT NOT NULL,            -- SPKI PEM
  relay_tunnel_id TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  last_seen_at INTEGER,
  revoked_at INTEGER
)

cloud_device_tokens(
  device_id TEXT PRIMARY KEY,
  token_hash TEXT NOT NULL,            -- SHA-256(deviceToken)
  created_at INTEGER NOT NULL,
  last_used_at INTEGER
)
```

- SQLite 路径：`MODE_CLOUD_DB_PATH`，容器默认 `/data/cloud.sqlite`；WAL 模式。
- 只保存微信 ID 的 HMAC；原文不落库、不写日志。

## 控制面 API

### `POST /v1/devices/enroll`

- 鉴权：`X-Mode-Enroll-Token` == `MODE_CLOUD_ENROLL_TOKEN`，每客户端 IP 10 次/分钟。
- 请求：`account{provider,providerUserId,displayName?}`、
  `device{id,name,platform,version?,publicKey,relayTunnelId}`、
  `assertion{payload,signature}`、`nonce`、`issuedAt`。
- `assertion.payload` 必须是 canonical JSON，且字段与请求一致；
  `issuedAt` 与服务器时间偏差 ≤ 5 分钟；签名用 Ed25519 公钥验证。
- 成功 200：`{accountId, deviceId, deviceToken}`；令牌只下发一次。
- 409：`relayTunnelId` 被其他设备占用；403：设备属于其他账号 / 账号禁用。

### `GET /v1/devices`

- 鉴权：`Authorization: Bearer <deviceToken>`。
- 返回 `{account:{id,displayName?}, devices:[{id,name,platform,version?,online,relayUrl,lastSeenAt?}]}`。
- `online = getTunnelIds().includes(relayTunnelId)`；
  `relayUrl = <publicBaseUrl>/d/<relayTunnelId>`。

### `POST /v1/devices/:id/revoke`

- 鉴权：设备令牌；只能撤销同账号设备。写 `revoked_at` 并删除目标设备令牌。
- 撤销自己时当前令牌立即失效。

### `GET /v1/health`

- 返回 `{ok, control, accounts, devices}`；用于部署验收。

## 客户端流程

### 左下角 Mode 账号入口

- `WorkspaceSidebarFooter` 头像菜单恢复账号分组（沿用原版顺序：偏好在上、账号/退出在底部）：
  - 未绑定：`登录 Mode 账号`；
  - 已绑定：`Mode 账号` + `退出账号`。
- 点击打开 `ModeAccountDialog`：内含账号状态、设备目录、6 位码配对入口和“绑定微信 Bot”。
- “绑定微信 Bot”打开现有 `BotsDialog`（微信通道）；绑定完成后由 Host 自动登录/同步。
- 头像/名称优先显示原 `user`，没有时显示云端账号名或已绑定微信名。

### 绑定后自动登录并同步

- `IBotsService.onDidChange` 在 `/bind`、`saveBot`、`removeBotSecret`、`deleteBot`、`saveConfig` 后触发。
- Host 的 `cloudAutoProvision` 只在**账号尚未绑定**（没有 device token）时工作：
  1. 找到启用且已绑定的微信 Bot；
  2. `bind()` 注册/登录设备；
  3. `syncCloudLoginBot` 上传该 Bot token 到云端并停用本地副本。
- 已绑定账号后不再自动动其他 Bot，避免误停用主 Bot；后续换 Bot 走账号弹窗里的手动同步。
- 自动流程依赖部署 enrollment secret；没有 secret 的机器应使用“使用微信登录”的 6 位码路径。
- 同步失败只记录 `cloud_auto_provision_failed`，不阻断 Bot/账号其他操作。

### 云端 Bot 登录（推荐，换机零配置）

1. Desktop `POST /v1/auth/start` → `{code, expiresAt}`（6 位、5 分钟、一次性、按 IP 限速）。
2. 用户在微信里把 code 发给云端 Bot。
3. 云端 Bot 校验发送者：
   - 已有绑定：哈希必须匹配；
   - 设置 `MODE_CLOUD_WECHAT_ALLOWED_USER`：只允许该微信用户；
   - 两者都没有：TOFU，首个用有效 code 完成验证的用户成为账号锚点。
   Bot 只把 code 标记为 `approved`，不回复消息。
4. Desktop `POST /v1/auth/poll`（code + 设备公钥 + relayTunnelId）：
   - `pending` → 202；
   - `approved` → 创建/查找账号，签发 `deviceToken` 与共享 `relayToken`，返回 200。
5. Desktop 保存令牌并调用 LAN 监听的 `startRelayTunnel(relayToken)` 启动隧道。

云端 Bot 必须使用**独立的 iLink bot token**（`MODE_CLOUD_WECHAT_TOKEN`）；与桌面 Bot
共用 token 会互相抢 `getupdates` 消息。新电脑装完应用后只需“使用微信登录”，不需要
部署 secret、relay token 或 `cloud-deployment.json`。

### 绑定（旧路径，保留为兜底）

1. 设置页确认已绑定微信 Bot。
2. 点击“绑定到 Mode Cloud”。
3. Host 读取 `serverId`、`relayTunnelId`（缺少则提示先开启远程控制/中继），
   生成/复用 Ed25519 密钥，构造 canonical payload 并签名。
4. `POST /v1/devices/enroll`；保存 `accountId` 与 `deviceToken`。
5. UI 重读目录，显示账号与设备列表。

### 配对

1. 主控在云设备列表选目标，输入被控端“远程控制”页显示的 6 位码。
2. Host 从目录取 `relayUrl`，解析为 `host/port/path`。
3. 调 `pairLanPeer({scheme:"https", path, code})` → 被控端 `/api/lan/pair`。
4. 成功后写入现有 `lan:peer:*` 记录；设备进入常规列表，连接按公网优先。

## 接口

- 共享契约：`packages/shared/src/cloudAccount.ts`。
- 服务契约：`packages/services/src/cloud-account/cloudAccountService.ts`（`cloud-account`，local-only）。
- `ICloudAccountService` 增加 `beginWechatLogin` / `pollWechatLogin` / `cancelWechatLogin`；
  登录成功后保存账号/设备/中继令牌，并调用 LAN 监听 `startRelayTunnel` 启动隧道。
- `IBotsService.onDidChange: Event<void>`：Bot 配置/绑定变更信号（不携带配置，消费方自行重读）。
- `cloudAutoProvision`：首次绑定 Bot 的自动登录/上传；`WorkspaceSidebarFooter` +
  `ModeAccountDialog` 是账号入口。
- 控制面：`packages/cloud-relay/src/cloudStore.ts`、`controlApi.ts`。
- 桌面实现：`packages/desktop/src/host/cloudDeviceIdentity.ts`、`cloudAccountService.ts`。
- UI：`packages/ui/src/remote-desktop/RemoteControlCloudSection.tsx`。

### 部署配置（推荐，避免逐机 env）

安装/构建时写入 `cloud-deployment.json`（权限 0600，加入 `.gitignore`）：

```json
{
  "cloudBaseUrl": "https://relay.mymode.cloud",
  "enrollToken": "<MODE_CLOUD_ENROLL_TOKEN>",
  "relayToken": "<MODE_RELAY_TOKEN>"
}
```

Desktop 读取顺序：

1. `MODE_CLOUD_CONFIG_FILE`
2. `<MODE_HOST_RESOURCES_DIR>/cloud-deployment.json`（打包资源）
3. `<appConfigDir>/cloud-deployment.json`（安装/开发机）
4. `<cwd>/cloud-deployment.json`

显式 env 优先于文件，便于测试/临时覆盖。文件同时提供云端地址、enrollment secret
和 relay token；Desktop 不再需要逐机 `setx`。

### 环境变量（仍支持，优先级更高）

| 变量 | 位置 | 说明 |
| --- | --- | --- |
| `MODE_CLOUD_ENROLL_TOKEN` | 云端 + 各 Desktop | 注册设备的部署 secret；缺失则控制面/绑定关闭 |
| `MODE_CLOUD_ACCOUNT_PEPPER` | 云端 | 微信 ID HMAC key；缺省复用 enrollment token |
| `MODE_CLOUD_DB_PATH` | 云端 | SQLite 路径，默认 `/data/cloud.sqlite` |
| `MODE_CLOUD_BASE_URL` | Desktop | 控制面地址，默认 `https://relay.mymode.cloud` |
| `MODE_CLOUD_CONFIG_FILE` | Desktop | 显式指定部署配置文件路径 |
| `MODE_CLOUD_WECHAT_TOKEN` | 云端 | 云端 Bot 的**独立** iLink token；缺失则 `/v1/auth/*` 关闭 |
| `MODE_CLOUD_WECHAT_ALLOWED_USER` | 云端 | 允许登录的微信用户 ID；缺省 TOFU 首个有效码 |
| `MODE_CLOUD_WECHAT_BASE_URL` | 云端 | iLink API 基址（测试/私有部署） |

## 安全边界

- 云端信任云端 Bot 的微信消息来源（iLink 长连接）与可选的 allowed user；
  `MODE_CLOUD_ENROLL_TOKEN` 只保留给旧路径/自动化注册，不再要求每台 Desktop 持有。
- `MODE_CLOUD_WECHAT_TOKEN` 是云端 Bot 的登录态 secret，泄漏等于可冒充 Bot；必须
  独立于桌面 Bot、定期轮换、不写日志/仓库。
- 设备令牌只存 SHA-256；微信 ID 只存 HMAC；日志只记录 `accountId`/`deviceId` 前缀。
- 配对仍由被控端 6 位码授权；云端不接触 LAN 令牌、票据或媒体内容。
- 云端登录成功会把共享 `MODE_RELAY_TOKEN` 下发给该设备（TLS 保护）；中继仍是单进程/
  单副本部署，目录不解决 tunnelId 抢占，每设备中继令牌留到后续。
- Cloudflare/Caddy 终止 TLS，云端能看到设备元数据与中继明文；E2E 仍未做。
- SQLite 需要备份；扩展后的 cloud-relay 不再是纯无状态进程。

## 验收

1. 被控端绑定微信 Bot → 绑定 Mode Cloud → 目录出现设备，在线状态与隧道一致。
2. 主控同一微信账号 → 看到设备与公网地址，不需要手填链接。
3. 主控选设备、输入被控端 6 位码 → 配对成功，设备进入现有列表。
4. 不同账号看不到彼此设备；错误/过期码返回 401；重复绑定被限速。
5. 撤销设备后目录不再返回；已配对 LAN 记录不受影响。
6. 中继容器重启 → SQLite 数据保留，隧道重连后在线状态恢复。
7. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 与相关测试全绿。
8. 新电脑安装后直接「使用微信登录」：显示 6 位码 → 发给云端 Bot → 自动绑定并启动隧道；
   不需要部署文件、enrollment secret 或 relay env。
9. 左下角 Mode 入口绑定微信 Bot 后自动登录并同步；设备目录出现该设备，云端登录 Bot 可用；
   本地该 Bot 停用，后续主 Bot 不被误停用。

## 非目标

- 微信官方 OAuth/身份验证；云端 Bot 依赖 iLink 长连接与 allowed user/TOFU。
- 同账号免码自动授权、账号恢复、多账号/组织。
- 每设备中继令牌、mTLS、E2E 加密。
- 手机/浏览器端完整远控 UI。
