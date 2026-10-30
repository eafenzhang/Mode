# Mode Cloud 个人账号体系（GitHub 登录 + 用户名密码 + 账号隔离公网远控）

状态：2026-10-30 拍板并实现（P0）。本 spec 取代
[mode-cloud-account-directory.md](./mode-cloud-account-directory.md) 中的账号锚点设计：
**微信登录（云端 Bot + `/v1/auth/start|poll`）已移除**，身份方式改为
**GitHub OAuth（Device Flow）+ 用户名密码注册**。开放问题结论：邀请码默认开放注册
（配置 `MODE_CLOUD_INVITE_CODE` 后强制，只约束密码注册；GitHub 身份经 GitHub 验证，
JIT 建号不设邀请码）；connect-ticket 免码留在 P2；账号删除放 P3。关联：
[wss-relay-mvp.md](./wss-relay-mvp.md)（中继传输）、
[lan-remote-public-endpoints.md](./lan-remote-public-endpoints.md)（公网端点）、
[remote-control-device-model.md](./remote-control-device-model.md)（统一设备模型）、
[product-line-naming.md](./product-line-naming.md)（产品线命名：Mode Cloud / Mode / Mode Go / Mode Bot）、
[user-profile-display.md](./user-profile-display.md)（账号展示）。

## 背景：现状与差距

现有 Mode Cloud（fnOS 上的 `packages/cloud-relay` 容器，SQLite + `/v1` 控制面）没有真正的账号体系：

1. **账号锚点 = 微信 Bot 绑定的用户 ID**（`cloud_accounts.provider='weixin'` + HMAC）。
   登录依赖 iLink 长连接与云端 Bot token 的可用性，换微信、Bot token 失效都会断登录。
2. **中继令牌是部署级共享 secret**（`MODE_RELAY_TOKEN`）：任何持有部署 secret 的设备
   都能以任意 `tunnelId` 注册隧道；无法按账号/设备撤销。
3. **主控端没有账号会话**：手机/浏览器必须知道 `/d/<tunnelId>` 地址并输入 6 位码。
4. deviceToken 长期有效直到 revoke，没有人类会话、过期、多端登录管理。

设备目录已按 `account_id` 过滤（`GET /v1/devices`），跨账号不可见的骨架存在。
本 spec 在其上补齐「GitHub 登录 + 用户名密码注册 + 会话管理 + 每设备中继令牌」，
使一个 fnOS 部署支持多个互相隔离的个人账号，任意主控登录后即可公网远控自己账号下的设备。

## 目标

1. **GitHub 登录**：部署方创建一个 GitHub OAuth App（开启 Device Flow），设备与主控
   都通过 GitHub Device Flow 登录（显示 user code → 用户在 `github.com/login/device`
   输入授权）；云端用 GitHub API 验证 access token，JIT 建号。
2. **用户名密码注册**：作为不依赖 GitHub 的兜底身份方式；部署方可选配置邀请码/关闭。
3. **账号隔离**：设备目录、中继隧道、远控入口按账号隔离；跨账号不可见、不可达。
4. **任意主控登录后公网远控**：桌面主控与手机/浏览器门户登录同一账号，看到自己账号的
   设备并远程控制（6 位配对码仍是首次控制授权）。
5. **被控端换机零配置**：新设备登录个人账号即可 enroll（不再依赖微信 Bot 与部署 secret）。
6. **每设备中继令牌**：隧道鉴权从部署共享 secret 改为每设备签发、可单独撤销。

## 非目标

- E2E 加密：仍为可信自建中继（Cloudflare 终止 TLS，中继可见明文）；上行 E2E 是
  开放给不可信中继的前置。
- 组织/团队/多成员共享设备（单账号即隔离域）。
- 邮箱/手机号注册与自动化找回密码（自部署无邮件服务；P3 可选 SMTP）。
- 微信登录（本期移除）：云端 Bot、`/v1/auth/start|poll`、自动登录与 Bot 同步全部下线；
  本地微信 IM Bot（`im-bot-*` specs）不受影响。
- 账号合并（同一人的 GitHub 与密码身份关联到同一账号）在 P0 不做。
- GitHub OAuth 网页回调流（redirect flow）：P0 只用 Device Flow（桌面 + 门户通用）。

## 身份方式与信任模型

| 方式 | 锚点 | 信任来源 | 适用 |
| --- | --- | --- | --- |
| GitHub 登录 | `provider='github'`，`provider_user_hash = HMAC(pepper, "github:" + GitHub 数字用户 ID)` | 云端持用户 access token 调 `GET https://api.github.com/user` 验证，id 以 GitHub 为准 | 首选；JIT 建号 |
| 用户名密码 | `provider='password'`，`provider_user_hash = HMAC(pepper, "password:" + username)`，username 明文小写存列 | scrypt 验证（N=2^15, r=8, p=1，每用户 salt） | 兜底；注册制 |

- **注册即登录**：密码注册成功直接返回会话；GitHub 首次登录 JIT 创建账号。
- 同一账号可同时具备两种方式：GitHub/微信迁移账号可通过「设置用户名密码」
  （`POST /v1/auth/credentials`，Bearer 会话或设备令牌）补设密码；密码登录按
  `username` 列查找账号（不限 provider）。
- 云端不保存 GitHub access token：验证用完即弃，不落库、不写日志。
- 迁移：存量 `provider='weixin'` 账号与设备数据保留。已绑定设备可用设备令牌调
  credentials 补设用户名密码；未补设且无法登录的旧账号，允许**换号接管**
  （见产品规则 8）。

## 产品规则

1. **注册（密码）**：
   - 用户名 3–32 字符，`[a-z0-9_-]`，写入前小写归一；保留名（`admin`、`mode`、`root`、
     `system`、`cloud` 等）拒绝注册。
   - 密码 8–128 字符；scrypt 哈希只存哈希（`node:crypto` 内置，无原生依赖）。
   - `MODE_CLOUD_INVITE_CODE` 配置时注册必须携带该邀请码；未配置时开放注册。
     注册接口按 IP 限速（10 次/分钟）。
2. **登录与会话**：
   - 登录成功签发 **sessionToken**（32 字节 base64url，服务端只存 SHA-256），
     30 天滑动过期（`MODE_CLOUD_SESSION_TTL_MS` 可调）；`refresh` 时轮转
     （旧 token 立即失效，新 token 下发）。
   - 登录接口按 IP 与按用户名双维限速（10 次/分钟 + 5 次/分钟）；用户名不存在与
     密码错误统一返回 401 `invalid_credentials`。
   - 会话可列出、可单独撤销（多端管理）；`logout` 撤销当前会话。
3. **GitHub 登录（Device Flow）**：
   - Desktop 发起 `POST https://github.com/login/device/code`（只需 public `client_id`，
     来自 `MODE_CLOUD_GITHUB_CLIENT_ID` 或部署配置 `githubClientId`；scope `read:user`），
     展示 `user_code` 与 `verification_uri`；轮询 GitHub token 端点
     （`authorization_pending` 继续等、`slow_down` 退避、`expired_token` 报错）。
   - 拿到 GitHub access token 后 Desktop 调 `POST /v1/auth/github {accessToken, clientLabel?}`；
     云端调 GitHub API 验证并用返回的数字 id 定位/创建账号，签发 sessionToken。
   - 云端验证 GitHub token 的 API 基址可用 `MODE_CLOUD_GITHUB_API_BASE_URL` 覆盖
     （代理/私有化）。
4. **设备归属**：
   - 被控端 enroll 为**账号登录态鉴权**：`POST /v1/devices/enroll` 接受
     `Authorization: Bearer <sessionToken>`，账号从会话解析，请求体不带账号锚点；
     Ed25519 设备断言校验保留（payload 只含 device/nonce/issuedAt）。
   - 设备与账号关系：同一 `serverId` 只属于一个账号；重复 enroll 更新元数据并重签
     deviceToken 与中继令牌。
   - **换号接管**：enroll 遇到设备属于其他账号时，若原属账号已无任何可用登录方式
     （`provider='weixin'` 且未补设密码——微信登录已移除），允许转移到当前登录账号，
     响应与日志标注 tookOver；其余情况维持 403 `device_conflict` 语义不变。
   - 旧 `X-Mode-Enroll-Token` 路径保留给部署自动化（运维工具），桌面 UI 不再使用。
5. **中继隧道**：
   - enroll/微信…所有登录路径的成功响应都下发**每设备中继令牌**（`relayToken`，
     服务端只存哈希于 `cloud_devices.relay_token_hash`）；`/_tunnel` 鉴权按 `tunnelId`
     查设备并校验该设备的令牌。
   - 未启用控制面的纯中继部署（无 SQLite 或无该 tunnelId 记录）回退共享
     `MODE_RELAY_TOKEN`，保持向后兼容。
   - revoke 设备即删除其中继令牌，中继**主动断开**该 tunnel（`closeTunnel` 回调）。
6. **主控目录**：`GET /v1/devices` 同时接受 sessionToken（人类主控）与 deviceToken
   （设备自查）；两者都按账号过滤；响应 `account` 携带 `username?`（用于 UI 判断
   是否提示「设置用户名密码」）。
7. **6 位配对码仍是远控授权**：账号登录只解决「找到自己的设备与地址」；首次对某台
   设备的远控仍需被控端 6 位码（`/api/lan/pair`，被控端本地授权），配对成功后沿用
   现有 `lan:peer:*` 令牌，后续免码。免码自动授权（connect-ticket）见分期 P2。
8. **展示**：显示名优先级沿用 [user-profile-display.md](./user-profile-display.md)，
   Cloud 来源为 `cloud_accounts.display_name`（GitHub 登录取 GitHub name/login）或
   本地昵称；侧栏与账号弹窗显示用户名或显示名。

## 状态所有者

| 状态 | 所有者 | 说明 |
| --- | --- | --- |
| 云账号（用户名/密码哈希/GitHub 锚点） | cloud-relay SQLite（`cloud_accounts`，扩展列） | 唯一持久化真相 |
| 人类会话 | cloud-relay SQLite（`cloud_user_sessions`，新增） | 服务端会话真相 |
| sessionToken | 主控端凭据服务（`cloud:session:token`）/ 门户 HttpOnly Cookie | 只是令牌副本 |
| 设备中继令牌哈希 | cloud-relay SQLite（`cloud_devices.relay_token_hash`，新增列） | 每设备一个 |
| 设备令牌 / Ed25519 私钥 / relayToken | 被控端凭据服务（`cloud:device:token` / `cloud:device:private-key` / `cloud:relay-token`） | 现有键复用 |
| GitHub Device Flow pending（device_code 等） | Desktop Host 进程内存 | 会话级；不持久化 |
| 隧道注册表 | cloud-relay 进程内存 | 不变；新增按 tunnelId 关闭能力 |
| 云目录快照 / 登录面板状态 | 页级瞬态 | 每次进入重读 |

不新增第二份设备列表或会话状态；桌面与门户都只消费 `/v1` 控制面。

## 数据模型与迁移

```sql
-- cloud_accounts 扩展（SQLite ALTER TABLE 逐列添加，v2 迁移）：
--   username      TEXT          明文小写用户名；GitHub 账号在补设前为 NULL
--   password_hash TEXT          scrypt 串；未设置密码为 NULL
--   updated_at    INTEGER
-- 用户名唯一性：密码账号由 UNIQUE(provider, provider_user_hash) 保证；
-- credentials 补设时显式查重。
CREATE TABLE IF NOT EXISTS cloud_user_sessions (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL,
  token_hash TEXT NOT NULL UNIQUE,
  client_label TEXT,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER,
  expires_at INTEGER NOT NULL,
  revoked_at INTEGER,
  FOREIGN KEY(account_id) REFERENCES cloud_accounts(id)
);
-- cloud_devices 扩展：
--   relay_token_hash TEXT        每设备中继令牌哈希
```

- 迁移版本 2 在容器启动时自动执行；存量 weixin 账号不改动即生效
  （username/password 为 NULL）。
- 微信登录移除后 `cloud_login_codes` / `cloud_bot_binding` / `cloud_bot_config`
  不再对新部署建表；存量库中的残留表与数据无害保留，不迁移。
- 密码、令牌只存哈希；日志只记 `accountId`/`deviceId`/会话 id 前缀。

## 控制面 API

### `POST /v1/auth/register`

- `{username, password, displayName?, inviteCode?}`；限速 10 次/分钟/IP。
- 校验：用户名格式与保留名、密码长度、邀请码（配置时）；用户名冲突 409 `username_taken`。
- 成功 200：`{sessionToken, expiresAt, account:{id, username, displayName?}}`（注册即登录）。

### `POST /v1/auth/github`

- `{accessToken, clientLabel?}`；限速 10 次/分钟/IP。
- 云端以该 token 调 GitHub `GET /user` 验证；401 `invalid_github_token`。
- 成功 200：`{sessionToken, expiresAt, account:{id, username?, displayName?}}`
  （账号不存在时 JIT 创建，display_name 取 GitHub name/login）。

### `POST /v1/auth/login`

- `{username, password, clientLabel?}`；双维限速（IP 10/分钟、用户名 5/分钟）。
- 按 `username` 列查找账号（不限 provider）→ scrypt 验证。
- 成功 200：`{sessionToken, expiresAt, account}`；账号禁用 403。

### `POST /v1/auth/refresh` / `POST /v1/auth/logout`

- Bearer 会话；refresh 轮转（旧会话 revoke + 新会话写入同一事务），
  失效 401 `session_expired`；logout 撤销当前会话。

### `GET /v1/auth/sessions` / `DELETE /v1/auth/sessions/:id`

- Bearer 会话（设备令牌不可列会话）；列表返回自己账号的活跃会话，可撤销任意自己的会话。

### `POST /v1/auth/credentials`

- Bearer 会话或设备令牌；`{username, password}`；给 GitHub/迁移账号补设密码登录方式。
- 用户名被占用 409；成功后该账号可用密码登录。

### `POST /v1/devices/enroll`（变更）

- 鉴权二选一：`Authorization: Bearer <sessionToken>`（新，桌面唯一路径）或
  `X-Mode-Enroll-Token`（运维自动化过渡保留）。
- 会话路径：请求体不含 `account` 块（携带即 400）；断言 payload 只含
  `device`/`nonce`/`issuedAt`。
- 成功响应：`{accountId, deviceId, deviceToken, relayToken}`（relayToken 每次轮换）。

### `GET /v1/devices` / `POST /v1/devices/:id/revoke`（变更）

- 鉴权二选一：deviceToken（现状）或 sessionToken；按账号过滤不变。
- revoke 额外：删除中继令牌 + `closeTunnel(tunnelId)` 断开在线隧道。

## 中继隧道鉴权改造（`server.ts` / `cloudRuntime.ts`）

```text
旧：/_tunnel?tunnelId&token  →  token == MODE_RELAY_TOKEN（部署共享 secret）
新：/_tunnel?tunnelId&token
      ├─ 控制面启用且 cloud_devices 有该 tunnelId → token == 该设备 relay_token_hash
      └─ 否则（纯中继/未知隧道）→ token == MODE_RELAY_TOKEN（向后兼容）
```

- `RelayCloudOptions` 注入 `verifyTunnelToken(tunnelId, token): boolean` 与
  `closeTunnel(tunnelId): void`（cloudRuntime 实现查库，server 不直接触库）。
- revoke 断连后，被控端隧道客户端按现有指数退避重连；因令牌已删，重连被 401 拒绝，
  客户端停止重连并在设置页显示「已被移除」。

## 客户端流程

### 被控端（Desktop / CLI，换机零配置）

```text
设置页「登录并绑定此设备」
  → 选择 GitHub 登录：Host 发起 Device Flow → 显示 user_code + 打开
    github.com/login/device → 用户授权 → Host 轮询到 access_token
  → POST /v1/auth/github（换 sessionToken）
  （或选择用户名密码：register / login）
  → POST /v1/devices/enroll（Bearer session + Ed25519 断言，无 account 块）
  → 保存 accountId / deviceToken / relayToken（凭据服务）
  → startRelayTunnel(relayToken) 启动隧道
```

- 未登录任何账号时不启动隧道，保持 LAN-only（现状语义）。
- 旧微信自动登录（`cloudAutoProvision`）与 Bot 同步（`cloudBotSync`）随微信登录移除；
  绑定本地微信 Bot 不再触发任何云操作。

### 桌面主控

- `ModeAccountDialog` / 远程控制页云分区：GitHub 登录（Device Flow 面板）与
  用户名密码登录/注册表单；已绑定但未设密码的账号显示「设置用户名密码」。
- sessionToken 存凭据服务（`cloud:session:token`）；过期/401 时账号状态回 `unbound`
  并提示重新登录，不自动用密码重试。
- 会话列表与撤销走 `/v1/auth/sessions`（P2 完善 UI）。

### 手机/浏览器门户（P1，2026-10-30 实现）

- cloud-relay 在 `/**`（`/`、`/portal`、`/portal.html`）serve 一个自包含静态页
  （登录/注册/GitHub 登录 + 设备列表，无构建依赖、无外部请求）；`/d/*` 转发优先级不变。
- **会话 Cookie**：登录/注册/GitHub 登录成功同时下发 `mode_cloud_session`
  （HttpOnly、SameSite=Lax、Path=/，`cloudBaseUrl` 为 https 时加 Secure）。
  `/v1/devices`、`/v1/auth/sessions`、revoke 等入口在无 Bearer 时回退读该 Cookie；
  Desktop 仍用 Bearer，互不影响。logout 同时清 Cookie。
- **GitHub 登录走 relay 代理**（GitHub OAuth 端点不返回 CORS 头，浏览器不能直连）：
  - `POST /v1/auth/github/device/start` → relay 用 `MODE_CLOUD_GITHUB_CLIENT_ID`
    向 GitHub 申请 device code，返回 `{loginId, userCode, verificationUri, expiresAt}`；
    device_code 只留在 relay 内存（最多 1000 条、10 分钟清理），不下发浏览器。
  - `POST /v1/auth/github/device/poll {loginId}` → `202 {status:"pending"}` /
    `200 {sessionToken, expiresAt, account}` + Set-Cookie / `401`（过期或失效）。
  - access token 只存在于 relay 进程内，验证 GitHub 用户后立即丢弃。
- 设备行展示名称/平台/在线状态，点击跳 `https://relay/d/<id>/` 进入现有
  `web-remote-replayable` 链路，首次仍输入被控端 6 位码（`mode_lite_token` Cookie
  由被控端按 `/d/<id>` 路径下发，与会话 Cookie 不冲突）。
- 门户为中文界面（与站点风格一致：无外部字体/CDN、跟随系统主题）。

## 事件顺序（关键流）

```text
① GitHub 登录（Device Flow）
Desktop ── POST github.com/login/device/code（client_id）──► GitHub
        ◄── {device_code, user_code, verification_uri}
用户在 github.com/login/device 输入 user_code 并授权
Desktop 轮询 github token 端点（interval/slow_down）
        ◄── access_token
Desktop ── POST /v1/auth/github {accessToken} ──► 云端 ── GET api.github.com/user ──► GitHub
        ◄── {sessionToken, account}（JIT 建号；access token 用完即弃）

② 密码注册/登录
客户端 ── POST /v1/auth/register|login ──► 控制面（限速 → scrypt verify）
        ◄── {sessionToken, expiresAt, account}

③ 被控端 enroll
Host ── POST /v1/devices/enroll（Bearer session + 断言）──► 控制面
     ◄── {accountId, deviceId, deviceToken, relayToken}
Host 保存凭据 → startRelayTunnel(relayToken)
隧道 ── WSS /_tunnel?tunnelId&token=relayToken ──► 中继（按设备校验）→ ready

④ 主控远控（首次）
主控 ── GET /v1/devices（Bearer session）──► 目录（只返回本账号设备 + relayUrl）
主控 ── POST https://relay/d/<id>/api/lan/pair + 6 位码 ──► 隧道转发 ──► 被控端
     ◄── {token, serverId, publicRelay} → 写 lan:peer:*，后续免码

⑤ 撤销 / 接管
主控 ── POST /v1/devices/:id/revoke ──► 控制面：删令牌 → closeTunnel → 重连 401 停止
新账号 enroll 旧设备（原属账号无登录方式）──► 控制面转移归属（tookOver）并重签令牌
```

## 失败语义

- 注册冲突 409；邀请码错误 403 `invite_required`；格式错误 400。
- 登录失败统一 401 `invalid_credentials`；GitHub token 无效 401 `invalid_github_token`；
  触发限速 429。
- Device Flow：`authorization_pending` 继续轮询；`slow_down` 间隔 +5s；`expired_token`
  提示重新发起；缺少 client_id 在 begin 时给出可读错误。
- 会话过期/撤销：目录与 enroll 返回 401 `session_expired`；客户端清凭据回 `unbound`。
- enroll 409 语义不变（tunnel_conflict / device_conflict）；账号 disabled 403；
  接管成功响应正常 200。
- 隧道令牌错误 401（升级失败）；revoke 后被控端重连收 401 并停止退避重试。
- GitHub API 不可达（网络/代理）：`/v1/auth/github` 返回 502 `github_unreachable`，
  客户端提示稍后重试；不影响密码登录。

## 安全边界

- 密码 scrypt（内存硬）+ 每用户 salt；登录错误文案不区分用户名/密码。
- 所有令牌（session/device/relay）服务端只存 SHA-256；原文只在签发响应出现一次。
- GitHub access token：Desktop 只在换取 session 前持有；云端只用一次即弃，不落库不写日志。
- 注册与登录限速公网暴露面；`MODE_CLOUD_INVITE_CODE` 是密码注册的准入闸门；
  GitHub JIT 依赖 GitHub 账号信任，部署方可后续加 allowed-logins 名单（P2）。
- 隧道令牌每设备化后，单个令牌泄露影响面收敛到单设备；部署 secret 退化为
  「纯中继部署 + 运维自动化」用途。
- 中继与 Cloudflare 仍可见转发明文（令牌、键鼠、剪贴板）：可信自建边界不变，
  上行 E2E 仍是开放给第三方前的硬前置。
- 门户 Cookie（P1）：HttpOnly + Secure + SameSite=Lax；反代不记录完整 query（既有要求）。

## 部署（fnOS）

- 复用 `packages/cloud-relay` 的 Dockerfile / docker-compose：控制面与中继同进程不变。
- **部署方一次性准备**：在 GitHub 创建 OAuth App（回调地址可留空，启用
  「Enable Device Flow」），把 `client_id` 写入部署配置（cloud-deployment.json 的
  `githubClientId` 或 Desktop env `MODE_CLOUD_GITHUB_CLIENT_ID`）；client_id 是公开值，
  不需要 client_secret（Device Flow 不使用）。
- 新增环境变量：
  - `MODE_CLOUD_INVITE_CODE`（可选；配置后密码注册需邀请码）
  - `MODE_CLOUD_SESSION_TTL_MS`（可选；默认 30 天）
  - `MODE_CLOUD_GITHUB_API_BASE_URL`（可选；GitHub API 代理基址）
  - `MODE_CLOUD_GITHUB_CLIENT_ID`（可选；门户 GitHub 登录必需，public 值）
  - `MODE_CLOUD_GITHUB_BASE_URL`（可选；GitHub device/token 端点基址，默认 `https://github.com`）
- 移除环境变量：`MODE_CLOUD_WECHAT_TOKEN`、`MODE_CLOUD_WECHAT_ALLOWED_USER`、
  `MODE_CLOUD_WECHAT_BASE_URL`（云端 Bot 已下线）。
- 迁移版本 2 在容器启动时自动执行；SQLite 继续挂 `/data/cloud.sqlite`（需备份，不变）。
- 手机门户静态页随包分发，无独立容器、无独立构建链（P1）。

## 分期

- **P0（账号与令牌面，服务端 + 桌面端）**：schema 迁移；register/login/github/refresh/
  logout/sessions/credentials API；enroll 会话鉴权 + 每设备 relayToken；`/_tunnel`
  按设备校验 + revoke 断隧道；桌面端 GitHub/密码登录、注册、enroll UI；微信登录与
  自动登录/Bot 同步移除；共享令牌回退兼容；换号接管。
- **P1（手机/浏览器门户）**：门户静态页（GitHub 代理登录 + 密码登录、设备列表、
  进入远控）；会话 Cookie；`/v1/auth/github/device/*` 代理端点。
- **P2（体验与加固）**：connect-ticket 免码授权（云端签发一次性短时票据，被控端经
  deviceToken 向云端验票，中继转发语义不变）；会话管理 UI 完善；TOTP 两步验证；
  GitHub allowed-logins 名单；微信锚点数据清理工具。
- **P3（可选）**：SMTP 找回密码；邀请码管理界面；审计日志；账号删除。

## 验收

1. 密码注册：新用户名注册即登录；重名 409；保留名/短密码/错误邀请码（配置时）拒绝；
   限速生效（429）。
2. 登录会话：密码错误与不存在用户名返回一致 401；refresh 轮转后旧 token 失效；
   logout 后 token 失效；会话列表与单独撤销可用。
3. GitHub 登录：有效的 GitHub token 换取会话并 JIT 建号（锚点为 GitHub 数字 id 的
   HMAC）；无效 token 401；GitHub API 不可达 502；同一天第二次登录不新建账号。
4. 被控端换机零配置：新装 Desktop 用 GitHub 或密码登录 → enroll 成功 → 隧道以每设备
   令牌上线 → 目录出现该设备；全程不需要部署 secret。
5. 账号隔离：账号 A 的主控看不到账号 B 的设备；B 的 `relayUrl` 转发到被控端后仍被
   LAN 令牌/6 位码拦截。
6. 每设备令牌：revoke 设备后其隧道被主动断开，重连 401 并停止重试；其他设备不受影响。
7. 迁移与接管：存量微信绑定设备可用设备令牌补设用户名密码后正常登录目录；无法补设的
   旧账号设备在新账号 enroll 时自动转移归属（tookOver），原账号数据保留。
8. 向后兼容：未启用控制面的纯中继部署，共享 `MODE_RELAY_TOKEN` 隧道照常工作。
9. UI：远程控制页/账号弹窗提供 GitHub 与密码两种登录入口；微信登录入口与自动登录
   承诺全部消失；绑定本地微信 Bot 不再触发云操作。
10. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 与相关测试全绿。

## 开放问题（实现前需拍板）

1. 邀请码默认值：公网部署推荐必配邀请码，但个人 fnOS 单人使用时开放注册 + 强密码
   是否足够？（结论：默认开放，README/部署文档强烈建议配置；GitHub JIT 不设邀请码。）
2. connect-ticket 免码是否提前到 P1？（结论：留 P2，避免 P0 增加被控端验票链路。）
3. 账号删除时机？（结论：P3。）
