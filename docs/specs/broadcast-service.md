# Spec：跨窗口广播与 opaque claim（Rust `broadcast-service`）

状态：2026-10-31 首版。对应 `packages/services/src/broadcast/broadcastService.ts` 在
`parentPort === null` 时的分支——Tauri 线 `host-standalone` 正是这样构造
（`remoteWorkspaceService.ts` 的 `parentPort: null`），因此没有跨 Host 中转。

## 1. 语义

- `send(message)`：校验 `{ channel, payload, sourceWindowId? }` 后分发给**本窗口**
  的 `onMessage` 订阅者；不跨窗口、不落盘。
- `acquireClaim(key)` / `commitClaim(lease)` / `releaseClaim(lease)` / `tryClaim(key)`：
  进程内 claim 状态机。
- `onMessage`：事件名与 TS 一致；适配器在订阅时注册回调，取消订阅时移除。

## 2. 消息校验

- `channel`：字符串，`trim()` 后非空；否则报 `Invalid broadcast message: channel is required`。
- `payload`：任意 JSON；缺省为 `null`。
- `sourceWindowId`：可选整数；小数/非数字报错。
- 未知键按 zod 的 strip 语义忽略。

## 3. claim 状态机

- 归一化：`key.trim()`；空 key → `{ status: "unavailable" }`。
- 已 `committed` 的 key → `{ status: "committed" }`。
- 已 `reserved` 的 key → `{ status: "busy", retryAfterMs }`，其中
  `retryAfterMs = max(0, min(250, expiresAt - now))`。
- 否则创建 `reserved` lease：token 形如 `local-<key>-<now>-<seq>`（TS 用
  `local-<uuid>`，对拍时归一为 `<token>`），TTL 5_000 ms，返回
  `{ status: "acquired", lease: { key, token } }`。
- `commitClaim`：仅当 key 仍为 `reserved` 且 token 匹配 → `committed`（永久）。
- `releaseClaim`：仅当仍为 `reserved` 且 token 匹配 → 删除；迟到的 token 不影响
  后来 winner。
- `tryClaim`：`acquire` 得到 `acquired` 时立即 `commit` 并返回 `true`，否则 `false`。
- 容量：最多 1_024 条；超限按插入序淘汰最旧；过期的 `reserved` 在每次操作时清理。

## 4. 已知差异 / 边界

1. **无跨窗口中转**：与 Tauri 线当前行为一致（`parentPort: null`）。若将来壳内需要
   真正的跨窗口广播，应把 Hub 放在壳的窗口管理器，而不是改本服务。
2. **token 形态**：TS 用随机 UUID，Rust 用 key + 毫秒 + 序号；只用于 commit/release
   匹配，不对外承诺格式。
3. **错误文本**：TS 的 zod 错误是结构化数组，Rust 返回单句；语义相同。

## 5. 验收

- `pnpm broadcast:fixtures` 生成 `crates/broadcast-service/fixtures/broadcast.json`
  （claim 生命周期、空 key、tryClaim、busy、committed 与 send）。
- `cargo test -p broadcast-service` 回放同一序列（token/retry 归一后逐项比较）。
- `crates/host-channels/tests/broadcast_channel.rs` 覆盖发送/订阅/反订阅与 5 个方法。
