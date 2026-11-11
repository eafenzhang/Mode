# 远程控制 v3 跨账号配对 v1

状态：2026-11-11 服务端、桌面 Host 确认面与浏览器 Client 已实现（扫码/确认 UI 待接）。
母规范：
[remote-control-v3.md](./remote-control-v3.md) §10。

## 1. 目标

两台不同账号的设备建立一条**仅设备**的授权关系：Host 侧生成一次性票，
Client 扫码/粘贴票确认，用户在 Host 本机输入 8 位验证码完成确认；两端换新凭证，
之后按普通 v3 连接流程使用（membership + Control/Relay）。

非目标：二维码 UI、同一账号内的免码配对（已有设备目录）、多设备批量配对、
Web 端配对入口。

## 2. 流程

```text
Host（已登录 A） --POST /pairing/sessions--> 服务端
   服务端存 ticket 的 SHA-256，返回 pairingId + ticket + expiresAt（180s）
Host 展示 ticket（二维码/短票）

Client（已登录 B） --POST /pairing/sessions/claim {ticket}--> 服务端
   服务端校验 ticket、状态、TTL；生成 8 位验证码（只存哈希）→
   返回 { pairingId, verificationCode, hostName, expiresAt }

用户在 Host 本机输入验证码 --POST /pairing/sessions/:id/confirm {code}--> 服务端
   服务端校验 code/attempts/TTL；建 device_only 账号 + membership；
   两台设备迁入该账号（Host inbound_only=1、Client pairing_restricted=1）；
   轮换 Host 凭证并返回新 token

Client --POST /pairing/sessions/:id/redeem--> 服务端
   客户端用旧 token 认证；服务端轮换 Client 凭证（一次性）；
   返回新 token 并标记 redeemed
```

## 3. 安全边界

- 服务端只保存 `ticket` 与验证码的 SHA-256；比较使用 timing-safe。
- 验证码 8 位数字、5 次错误后作废；会话 180 秒；`confirm`/`redeem` 各一次。
- 设备身份只来自 access token，不信任请求体自报的 deviceId。
- Host 确认只能在本机 UI 发起；模型工具/远程 Client 没有 Host 的 access token，
  不能确认。
- 配对后：Host `inbound_only=1`（只能被连，不能主动连出）；Client
  `pairing_restricted=1`（无 Web/Admin 能力）；账号 provider=`device`。
- 不写明文 token/验证码到日志；响应中的 token 只由对应设备领取一次。

## 4. 数据

`pairing_sessions`（schema v2）：

| 列 | 说明 |
| --- | --- |
| `id` | pairingId |
| `ticket_hash` | ticket 的 SHA-256（唯一） |
| `host_device_id` | 发起方 |
| `client_device_id` | 认领方（claim 后写入） |
| `code_hash` | 8 位验证码哈希（claim 时生成） |
| `status` | `pending` / `claimed` / `confirmed` / `canceled` |
| `attempts` | 错误验证码次数（≥5 作废） |
| `created_at` / `expires_at` | TTL 180s |
| `claimed_at` / `confirmed_at` / `redeemed_at` | 状态时间线 |

## 5. API（全部走既有 device token 鉴权）

```text
POST /api/v1/pairing/sessions                  host 设备
  -> 201 { pairingId, ticket, expiresAt }

POST /api/v1/pairing/sessions/claim            client 设备
  body { ticket }
  -> 200 { pairingId, verificationCode, hostName, expiresAt }

GET  /api/v1/pairing/sessions/:id              参与方设备
  -> 200 { status, expiresAt, hostName?, clientName? }

POST /api/v1/pairing/sessions/:id/confirm      host 设备
  body { code }
  -> 200 { status: "confirmed", accessToken, refreshToken, accessExpiresAt, refreshExpiresAt }

POST /api/v1/pairing/sessions/:id/redeem       client 设备
  -> 200 { accessToken, refreshToken, accessExpiresAt, refreshExpiresAt }

POST /api/v1/pairing/sessions/:id/cancel       参与方设备
  -> 200 { status: "canceled" }
```

错误码：`pairing_invalid_ticket`、`pairing_expired`、`pairing_not_claimed`、
`pairing_code_invalid`、`pairing_attempts_exhausted`、`pairing_already_used`、
`same_account`、`pairing_not_confirmed`。

## 6. 状态与事件顺序

```text
pending --claim--> claimed --confirm--> confirmed --redeem--> (终态)
   \                    \
    `--cancel/expired----`--cancel/expired--> canceled/终态
```

- `claim` 与 `confirm` 互斥事务：先写状态再返回；重复 confirm 返回
  `pairing_already_used`。
- 设备迁移与 membership 在同一个事务里完成；任一步失败整体回滚。
- 过期按 `expires_at` 惰性判定，不后台扫表。

## 7. 验收

1. Host 建会话、Client claim 拿到 8 位码；重复 claim 拒绝。
2. 错误码 5 次后会话作废；正确码在 180s 内成功。
3. confirm 后两台设备 `account_id` 迁到 `device` 账号，`inbound_only` /
   `pairing_restricted` 置位，membership 可查；Host 旧 token 失效、新 token 可用。
4. Client redeem 一次成功、二次拒绝；旧 token 失效。
5. 过期会话、跨角色（client 建会话）与同账号配对被拒绝。

## 8. 增量

- 服务端：schema v2、`PairingStore` 事务、5 条 API、E2E 测试。
- 浏览器 Client SDK：`WebAccountClient.claimPairing/getPairing/redeemPairing/cancelPairing`
  （device token + 401 刷新重试；redeem 后本地 profile 立即换新 token）。
- 桌面 Host：`IRemoteV3Service.beginPairing/confirmPairing/getPairing/cancelPairing`
  已接；确认成功轮换 Host 凭证并重建 Control 链路。
- 浏览器：`claimPairing/getPairing/redeemPairing/cancelPairing` 已接；
  redeem 后刷新服务侧 profile 快照。
- E2E：真实 remote-server + 桌面 Host 服务 + `WebAccountClient`，覆盖
  begin → claim → confirm → redeem → 目录可见，以及取消后 claim 失败。
- 待接：二维码/短票展示与 Host 本机验证码输入 UI。
