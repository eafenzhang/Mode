# Mode Cloud 个人账号体系 v2（仅 GitHub 登录 + 账号即信任 + 目录直连）

状态：2026-10-30 晚拍板并实现（v2）。本 spec 取代本文 v1（GitHub + 用户名密码 +
6 位配对码）的设计：**密码注册/登录、邀请码、6 位配对码（远程控制路径）全部移除**，
身份只剩 **GitHub OAuth Device Flow**；**同账号即信任**，主控端从设备目录直接取得
被控端访问令牌，不再有配对码交互。关联：
[wss-relay-mvp.md](./wss-relay-mvp.md)（中继传输）、
[lan-remote-public-endpoints.md](./lan-remote-public-endpoints.md)（公网端点）、
[remote-control-device-model.md](./remote-control-device-model.md)（统一设备模型）、
[remote-control-unified-pairing.md](./remote-control-unified-pairing.md)（配对码仅剩
远程工作区等非远控场景）、[product-line-naming.md](./product-line-naming.md)、
[user-profile-display.md](./user-profile-display.md)。

## 背景与决策

v1 上线后（2026-10-30）实际使用反馈：注册表单、邀请码、6 位配对码都是流程负担。
个人自部署场景下「账号成员资格」本身就是信任边界，据此拍板：

1. **取消账号注册**：不再提供用户名密码注册/登录/补设（`/v1/auth/register|login|
   credentials` 下线），身份方式只有 GitHub Device Flow。
2. **重做登录交互**：发起登录即自动打开浏览器授权页（桌面 `openExternal`、门户自动
   跳转），登录端自动轮询，授权完成后**自动回到已登录状态**，全程无需手动刷新或输入。
3. **取消远程控制配对码**：设备对同账号成员自报访问令牌，目录把令牌下发给同账号的
   已登录主控；连接 = 读目录 → 直连，不再输入 6 位码。
4. **无旧版兼容**：v1 的密码列、微信接管、`X-Mode-Enroll-Token` 运维路径随本次删除；
   旧客户端（≤0.0.107）对新云端不兼容，直接升级。

## 目标

1. **仅 GitHub 登录**：云端用 GitHub API 验证 access token（`GET /user`），锚点为
   GitHub 数字用户 id 的 HMAC，JIT 建号；access token 用完即弃。
2. **账号即信任**：同一账号下的设备互相可见、可连；跨账号不可见、不可达。
3. **目录直连**：目录条目包含被控端访问令牌（云端加密存储、仅同账号可读）与地址
   （公网中继 URL + 自报 LAN 地址）；主控端无需任何配对交互即可连接。
4. **自动授权回登录**：Device Flow 发起后自动打开 `github.com/login/device`，登录端
   自动轮询（尊重 `interval`/`slow_down`），成功后状态机自动回到 bound。
5. **流程极简**：被控端 = 运行 exe → 显示 user code（自动开浏览器）→ 授权后自动
   enroll 上线；主控端 = 登录 → 设备列表点「连接」。

## 非目标

- E2E 加密：仍为可信自建中继（Cloudflare 终止 TLS）；中继可见转发明文。
- 组织/团队共享；邮箱/手机注册；账号删除（P3）。
- Mode Go 的云端 WSS 隧道（被控端仍需 LAN/Tailscale/公网可达，地址随 enroll 上报；
  上隧道是后续项）。
- 远程工作区（desktop↔desktop 挂接）的 6 位码：属远程工作区 spec，不在本次范围。

## 身份与信任模型

- 账号锚点：`provider='github'`，`provider_user_hash = HMAC(pepper, "github:" +
  GitHub 数字用户 id)`。唯一身份方式。
- 会话：`sessionToken`（32B base64url，服务端只存 SHA-256），30 天滑动
  （`MODE_CLOUD_SESSION_TTL_MS`），refresh 轮转，logout/sessions 管理保留。
- 云端不保存 GitHub access token：验证用完即弃。
- **设备访问令牌（新）**：设备 enroll 时上报自己的本机 API 令牌（`accessToken`，
  Mode Go 与 Desktop 同语义：凭它访问该设备的 HTTP API）。云端以 AES-256-GCM 加密
  落库（`cloud_devices.access_token_cipher`），**仅同账号已登录主控**经目录读回明文。
  加密密钥来自 `MODE_CLOUD_DB_KEY`，未配置时在数据目录自动生成 `cloud-db.key`
  （0600，随机 32B）；密钥丢失 = 目录令牌不可解密，设备重新 enroll 即恢复。

## 产品规则

1. **登录（唯一路径）**：Desktop / Mode Go / 门户都走 GitHub Device Flow：
   `POST github.com/login/device/code`（public client_id，scope `read:user`）→
   用户授权 → 轮询 token 端点（`authorization_pending` 继续等、`slow_down` +5s、
   `expired_token` 重新发起）→ `POST /v1/auth/github {accessToken}` 换云会话。
   发起成功即自动打开 `verification_uri`。
2. **设备归属**：
   - enroll 为账号登录态鉴权（`Bearer <sessionToken>`），Ed25519 设备断言保留
     （payload 只含 device/nonce/issuedAt）。
   - 请求体 `device` 块：`id/name/platform/version/publicKey/relayTunnelId/
     accessToken/addresses?`。`accessToken`（16–256 字符）必填；`relayTunnelId` 保留
     必填（暂未上隧道的设备生成一个但不连接，relayUrl 打开会 503）；`addresses`
     为设备自报可直连地址数组（`host:port`，≤8 个，可选）。
   - 重复 enroll：更新元数据/令牌/地址并重签 deviceToken（+有隧道时 relayToken）。
   - 归属冲突：设备属于其他账号默认 403 `device_conflict`；唯一例外是原属账号为
     旧身份（provider ≠ github，密码/微信已删除、无法登录的死账号）的遗留设备——
     允许首个 GitHub 账号认领（tookOver），否则升级部署后存量设备会被永久锁死。
   - `X-Mode-Enroll-Token` 运维路径删除。
3. **中继隧道**：不变——每设备 `relayToken` 只存哈希，`/_tunnel` 按设备校验，revoke
   主动断连；纯中继部署回退 `MODE_RELAY_TOKEN`。
4. **目录（连接的事实源）**：`GET /v1/devices`（sessionToken/deviceToken/Cookie）
   返回本账号设备：`{id,name,platform,version?,online,relayUrl,addresses?,
   accessToken,lastSeenAt?}`。`accessToken` 为解密后的设备 API 令牌，**只发给同账号**。
5. **连接（替代配对）**：主控端 `connectDevice(deviceId)` = 读目录 → 取
   `accessToken + relayUrl + addresses` → 写入既有 `lan:peer:*` 凭据（公网端点
   relayUrl 优先、LAN 地址在后）→ 设备进入既有已配对列表，后续复用现有连接/探测。
   无网络交互、无用户输入。
6. **展示**：显示名取 `cloud_accounts.display_name`（GitHub name/login）或本地昵称。

## 状态所有者

| 状态 | 所有者 | 说明 |
| --- | --- | --- |
| 云账号（GitHub 锚点） | cloud-relay SQLite（`cloud_accounts`） | v3 起无密码列 |
| 人类会话 | cloud-relay SQLite（`cloud_user_sessions`） | 不变 |
| 设备访问令牌（密文） | cloud-relay SQLite（`cloud_devices.access_token_cipher`） | AES-256-GCM |
| 加密密钥 | env `MODE_CLOUD_DB_KEY` 或 `<db 目录>/cloud-db.key`（0600） | 云端持有 |
| 设备中继令牌哈希 | `cloud_devices.relay_token_hash` | 不变 |
| sessionToken / deviceToken / Ed25519 私钥 / relayToken | 客户端凭据服务 | 键名不变 |
| Device Flow pending | 登录端进程内存（Desktop Host / relay 代理） | 不持久化 |
| 隧道注册表 | cloud-relay 进程内存 | 不变 |

## 控制面 API（v2）

- `POST /v1/auth/github`：`{accessToken, clientLabel?}` → `{sessionToken, expiresAt,
  account:{id, displayName?}}`（JIT 建号）。
- `POST /v1/auth/refresh` / `POST /v1/auth/logout` / `GET /v1/auth/sessions` /
  `DELETE /v1/auth/sessions/:id`：不变。
- `POST /v1/auth/github/device/start|poll`：门户 Device Flow 代理，不变。
- `POST /v1/devices/enroll`：会话鉴权；`device.accessToken` 必填、`addresses?`；
  响应 `{accountId, deviceId, deviceToken, relayToken?}`。
- `GET /v1/devices` / `POST /v1/devices/:id/revoke`：目录含 `accessToken`/`addresses`；
  revoke 语义不变。
- **下线**：`/v1/auth/register`、`/v1/auth/login`、`/v1/auth/credentials`（404）。

## 客户端流程

### 被控端（Desktop / Mode Go，同一套）

```text
运行 → 无有效会话 → Device Flow：申请 device code → 面板/门户显示 user_code
  → 自动打开 github.com/login/device → 用户授权 → 轮询拿 access_token
→ POST /v1/auth/github → sessionToken
→ POST /v1/devices/enroll（Bearer session + Ed25519 断言
   + accessToken=本机 API 令牌 + addresses=自报地址）
→ 保存 accountId/deviceToken/relayToken →（Desktop）startRelayTunnel
→ 目录可见，主控零交互可连
```

Mode Go 的 6 位配对/`enroll`/`pair` 命令与配对对话框删除；安装完成后直接进入
GitHub 登录。

### 桌面主控

- 远程控制页云分区 / 账号弹窗：未登录时展示「使用 GitHub 登录」→ 发起即自动开浏览器
  + 显示 user_code + 自动轮询，成功自动回到设备列表；不再有注册表单、密码面板、
  「设置用户名密码」。
- 设备行按钮「连接」：`connectDevice` 写入 `lan:peer:*` 后进入既有列表；无 6 位码输入。
- 远程控制页的「生成配对码」面板与 LAN 发现配对行移除（被控端经账号自助上线）。

### 手机/浏览器门户

- 打开即自动开始 Device Flow（显示 code 并自动跳转授权页），授权回来自动进入设备
  列表；登录/注册表单删除。
- 设备行「打开远控」= `relayUrl?token=<accessToken>`（`/d/<tunnelId>` 转发到被控端，
  被控端按既有 token 语义放行），不再需要 6 位码。

## 事件顺序（关键流）

```text
① 登录（三端同一）
端 ── POST github.com/login/device/code ──► GitHub ──► {device_code, user_code, uri}
端 自动打开 verification_uri；用户授权
端 轮询 token 端点 ──► access_token ── POST /v1/auth/github ──► 云端验 GitHub ──► {sessionToken}

② 设备自助上线
端 ── POST /v1/devices/enroll（session + 断言 + accessToken + addresses）──► 云端
   （AES 加密 accessToken 落库；重签 deviceToken/relayToken）
Desktop ── WSS /_tunnel（relayToken）──► 中继 ready

③ 主控直连（无配对）
主控 ── GET /v1/devices ──► {relayUrl, addresses, accessToken}
主控 ── 写 lan:peer:*（public=relayUrl、lan=addresses、token=accessToken）
     ── HTTPS relay/d/<id>/…（Bearer accessToken）──► 被控端

④ 撤销
主控 ── POST /v1/devices/:id/revoke ──► 删令牌（含 access_token_cipher）→ closeTunnel
```

## 失败语义

- GitHub token 无效 401 `invalid_github_token`；GitHub API 不可达 502
  `github_unreachable`；限速 429。
- Device Flow：pending 继续轮询；slow_down +5s；expired 重新发起；缺 client_id 在
  begin 时可读报错。
- 会话过期：目录/enroll 401 `session_expired`，客户端清凭据回 unbound。
- enroll：tunnel_conflict / device_conflict 409 语义保留（仅 GitHub 账号之间拒绝）。
- access_token_cipher 解密失败（密钥丢失）：目录该设备不带 `accessToken` 并在
  `tokenAvailable:false` 标注，主控提示「设备需重新登录」；设备重新 enroll 自愈。
- 密码/邀请码相关错误码全部删除。

## 安全边界

- 所有云端签发令牌只存 SHA-256；设备自报 accessToken 以 AES-256-GCM 加密存储，
  密钥不出部署机；DB 泄露不再直接得到可重放的远控令牌。
- 目录下发 accessToken 仅经 HTTPS + 同账号会话/Cookie；中继与 Cloudflare 仍可见
  转发明文（可信自建边界不变）。
- 账号即信任：被控端令牌泄露影响面 = 单设备；revoke 即收回（目录删除 + 隧道断开）。
- GitHub JIT 依赖 GitHub 账号信任；部署方可用 GitHub API 代理（原 env 保留）。

## 部署（fnOS）

- 环境变量新增：`MODE_CLOUD_DB_KEY`（可选；不配置则数据目录自动生成 `cloud-db.key`）。
- 环境变量移除：`MODE_CLOUD_INVITE_CODE`。
- 保留：`MODE_CLOUD_SESSION_TTL_MS`、`MODE_CLOUD_GITHUB_API_BASE_URL`、
  `MODE_CLOUD_GITHUB_CLIENT_ID`、`MODE_CLOUD_GITHUB_BASE_URL`。
- 迁移版本 3 自动执行：`cloud_devices` 加 `access_token_cipher`/`addresses` 列；
  `cloud_accounts` 的 `username`/`password_hash` 列删除（SQLite DROP COLUMN，失败时
  保留为无害死列）。

## 分期

- **v2（本次）**：GitHub-only 认证面；enroll/目录令牌化；门户与桌面自动授权 UX；
  远程控制页去配对；Mode Go 自助上线（GitHub 登录 + enroll）；密码/邀请码/接管删除。
- **后续**：Mode Go 云端 WSS 隧道；GitHub allowed-logins 名单；会话管理 UI；
  TOTP；账号删除；审计日志。

## 验收

1. 仅 GitHub：register/login/credentials 返回 404；GitHub token 换会话 + JIT 建号；
   同一 GitHub 账号重复登录不新建账号；无效 token 401。
2. 自动授权：桌面 begin 即打开浏览器；轮询由 Host 驱动（interval/slow_down），授权后
   UI 无需操作自动进入 bound；门户打开即出码并自动跳转。
3. 设备自助上线：Mode Go `mode-agent.exe login` 显示 user_code → 授权后自动 enroll，
   目录出现该设备（含 addresses 与 accessToken）；Desktop enroll 语义不变但携带
   accessToken。
4. 目录直连：主控登录后对目录设备点「连接」即写入 `lan:peer:*` 并出现在已配对列表，
   全程无 6 位码；跨账号读不到 `accessToken`（目录按账号过滤本来就不可见）。
5. 加密落库：SQLite 中 `access_token_cipher` 非明文；密钥文件缺失时目录降级为
   `tokenAvailable:false` 且不崩溃。
6. 撤销：revoke 后目录移除、隧道断开、重连 401。
7. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 与相关测试全绿。
