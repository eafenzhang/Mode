# Spec：对话镜像（`crates/conversation-mirror`）

状态：2026-10-29 实现（数据层；频道适配器待接）。行为真源：
`packages/shared/src/conversation-mirror.ts`、`packages/services/src/conversation-mirror/*`。
产品语义见 `docs/specs/remote-workspace-conversation-mirror.md`。

## 落盘布局

```
{dataRoot}/v2/conversation-mirror/<workspaceHash>/<sanitized originDeviceId>.json
```

- `workspaceHash` = `workspace_hash(normalize_workspace_path_for_identity(workspacePath))`
  （见 `workspace-identity.md`）。
- 文件名把 `[A-Za-z0-9._-]` 之外的字符换成 `_`。一个 origin 一个文件：重复发布只覆盖自己的。
- 读取：目录缺失/不可读返回空；**单文件损坏或校验失败只跳过该文件**（fail-soft）。
- 写入走 `atomic-file`（锁 + 临时文件 + rename），内容是紧凑 JSON（与 `JSON.stringify` 同形）。

## 快照校验

对应 zod `.strict()`：

- 顶层字段恰为 `version/originDeviceId/originLabel/generatedAt/workspacePath/records`，
  未知字段直接判无效；`version` 必须为 `1`。
- `originDeviceId` / `originLabel` / `workspacePath` / `sessionId` 会 **trim**，trim 后为空无效。
- record 恰为 `sessionId/title/createdAt/updatedAt/messageCount/truncated/messages`；
  message 恰为 `role/text/timestamp?`，`role` 只允许 `user` / `assistant`。
- 时间戳为无符号整数。

## origin 规则

| 入口 | 要求 | 不满足时 |
| --- | --- | --- |
| `writeLocalSnapshot` | origin **等于**本机 deviceId | `foreign-origin` |
| `publishSnapshot` | origin **不等于**本机 deviceId | `self-origin` |

校验失败统一 `invalid-record`；成功 `stored`，`recordCount` 是有界化后的记录数。
身份来自 `device-mid`（`{dataRoot}/v2/telemetry-state.json`），展示名取
`COMPUTERNAME` / `HOSTNAME`，trim 后为空用 `Mode`，最多 64 个字符。

## 有界化（顺序固定）

1. 同一 origin 内按 `sessionId` 去重，`updatedAt` 较新的胜出，位置保持首次出现；
2. 按 `updatedAt` 降序、截断到 **20 条**；
3. 每条记录：保留末尾 **200 条**消息；单条文本截断到 **4000 个 UTF-16 码元** + `…`
   （JS `slice` 口径）；再按 **256 KiB** 字节预算从最旧消息开始丢弃（至少留 1 条）；
4. 整份快照按 **512 KiB** 字节预算从最旧记录开始丢弃（至少留 1 条）。

字节数用 `serde_json::to_vec` 的紧凑输出长度，与 TS 的 `JSON.stringify` 对齐。

## 与 TS 的差异

- 展示名读环境变量而非 `os.hostname()`（Rust std 无跨平台 API）；两者在正常环境下一致。
- 时间戳用 `u64` 反序列化：JSON 里的 `1.0` 会被拒绝，而 zod 接受整数值浮点（边缘输入）。

## 验收

`crates/conversation-mirror`：7 条集成用例（校验 trim/严格字段/版本/角色、去重保留新值、
文本省略号截断、尾部消息保留、快照排序与上限、origin 两条入口与读回、坏文件跳过）+
1 条文件名清洗单测。`clippy -D warnings` 通过。
