# Spec：LAN 配对核心（Rust `lan-pairing`）

状态：2026-11-05 第一版（配对码 / 令牌 / 客户端记录）。
真源：`packages/desktop/src/host/lanAccessServer.ts` 与
`packages/desktop/src/host/lanAccessRecords.ts`；契约见 `packages/shared/src/lanAccess.ts`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `LanPairing::create_pair_code` / `current_pair_code` | `createPairCode` / `getState().pairCode` |
| `LanPairing::redeem` | `consumePairCode` + `redeem` |
| `LanPairing::verify_token` | `verifyAuthToken` / `resolveLanClientByToken` |
| `LanPairing::remove_client` / `reset_tokens` | 同名方法 |
| `LanPairing::record_client_workspace` / `set_remote_desktop_authorized` | 同名写路径 |
| `constant_time_equal` / `mint_client_token` / `pair_code_from_hex` | `constantTimeEqual` / `randomBytes(32).toString("base64url")` / `randomBytes(3).toString("hex")` |

**不做**：HTTP/WS 服务、UDP 发现、公网中继、凭据加密与文件 IO。它们由宿主装配；
本 crate 只通过 `LanCredentialPort` 读写凭据值。

## 2. 行为

- 配对码：6 位 hex 转大写；TTL 5 分钟；同一时刻只保留一个码。
  - 输入先 trim 再大写；比较用定长常量时间。
  - 过期码被清除；**错误码不消费当前码**；命中后无论后续写盘是否成功都作废。
- 令牌：32 字节随机源的 base64url（无 padding）；每个客户端一条，存
  `lan:access:client:<clientId>:token`。
- 客户端记录：`lan:access:clients` JSON 数组；读取宽松（非法 JSON/非数组整组按空，
  缺 `id` 条目跳过），`remoteDesktopAuthorized` 只认显式 `true`。
- serverId：`lan:access:server-id`，缺失时用注入的随机源生成并落盘。
- `verify_token`：遍历令牌常量时间比较；命中后按 60s 节流更新 `lastUsedAt`
  （写盘失败不影响鉴权结果）。
- `remove_client`：不存在为空操作；`reset_tokens` 删除全部令牌并清空记录。

## 3. 差异

1. 随机源、时钟、UUID 全部注入；默认实现由宿主提供（安全随机不得回退到时间戳）。
2. 写盘错误：`redeem`/`remove_client`/`reset_tokens` 返回 `Err(String)`；`touch` 的
   `lastUsedAt` 写盘失败被忽略（与 TS 的 catch/log 一致）。
3. 记录读取的 `createdAt` 缺失按 0（TS 用 `Date.now()`）；正常记录都会写该字段。

## 4. 验收

- `cargo test -p lan-pairing`：7 条（码大小写与单次消费、错误码不消费、过期拒绝、
  令牌验证/节流、撤销与重置、base64url 形状、宽松记录解析）。
- 与 TS 的行为对拍由后续 HTTP 接线与设置页验收覆盖。
