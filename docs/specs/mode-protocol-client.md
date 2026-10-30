# Spec：Mode Protocol 客户端内核（`crates/mode-protocol-client`）

状态：2026-10-29 实现（W4 前置第一片）。行为真源：

- `packages/services/src/mode-agent/modeProtocolClient.ts`（请求 id、pending、超时、通知）
- `packages/services/src/mode-agent/modeStdioTransport.ts`（LF 分帧与 EOF 行为）
- `packages/shared/src/mode-protocol/index.ts`（Request / Response / Error / Notification 形状）

## 范围

本 crate 只做**与进程所有权无关**的协议内核：

- 任意 `AsyncRead + AsyncWrite` 双向流上的 LF 分帧；
- 请求 id（整数或字符串）多路复用、单请求超时、关闭时统一失败；
- 通知广播；
- Agent → 客户端方向请求的默认应答（无 handler 时 `-32601 Method not found`）；
- 显式 `dispose` 中止读写任务。

**不在范围**（后续切片）：

- 拉起/终止 `mode app-server --stdio` 进程、进程树清理、stderr 收集、stdio tap；
- `ModeStorageStartupGate`（请求前置的存储启动门）；
- `lifecycle: observation`；
- 按方法的 zod/强类型结果校验（调用方用 `serde_json::Value` 自行解析）。

## 空闲计时（对应 `onPendingRequestsDrained`）

- 连接建立时记录 `last_drained = now`。
- 任一 pending 被移除（成功、服务端错误、超时、写失败）后，若 pending 已空则把
  `last_drained` 置为 `now`；移除与「是否为空」在同一把锁内判断，避免与新请求插入交错。
- 连接关闭（EOF/`dispose`）时 pending 全部失败，同样视为归零。
- `pending_count()` 与 `idle_duration()` 一起构成空闲判定：只有在飞请求为 0 且空闲时长达到
  阈值才允许回收。注册表的 `reap_idle` / `spawn_idle_reaper` 使用这组语义
  （见 `docs/specs/agent-process.md`）。

## 帧

- 帧边界**只认 `\n`**：U+2028/U+2029 是 JSON 字符串里的普通字符，不能当换行。
- 读到 EOF 时，残留的非空缓冲区按一帧处理（与 TS `handleStdoutEnd` 一致），随后连接关闭。
- 容忍 CRLF（解析前去掉行尾 `\r`）；空行忽略。
- 单帧超过 64 MiB 直接按协议错误关闭（TS 没有显式上限；这是 Rust 侧的保护）。
- 非法 JSON / 非对象帧 / 既无 id 也无 method 的帧：按协议错误关闭并失败所有 pending。

## 消息分派

| 形状 | 处理 |
| --- | --- |
| `{id, result}` | 完成对应 pending |
| `{id, error:{code,message,data?}}` | 以 `Server { code, message, data }` 失败对应 pending |
| `{id, method, params?}` | 交给 handler；无 handler 或返回 `None` → 回 `-32601` |
| `{method, params?}` | 广播给通知订阅者 |

未知 id 的响应/错误被忽略（对端可能已超时/被取消）。

## 请求与超时

- 请求 id 从 1 递增（`i64`）；响应按 id 匹配。
- 默认超时 180s（与 TS 一致），可按请求覆盖；超时只失败该请求，并从 pending 移除。
- 连接关闭（EOF、写失败、`dispose`）时，所有 pending 以 `Closed` 失败。

## 已知差异

| 差异 | 说明 |
| --- | --- |
| 结果校验 | TS 客户端支持按方法传 zod schema；这里只返回 `Value`，由调用方解析 |
| 存储启动门 | TS 的 `ModeStorageStartupGate` 未移植 |
| 观测语义 | `lifecycle: observation` 未移植；pending 计数与空闲时长已提供（`pending_count` / `idle_duration`） |
| 帧上限 | Rust 增加 64 MiB 上限；正常协议帧远小于此 |
| 进程生命周期 | 本 crate 不拥有进程；`dispose` 只中止本地读写任务 |

## 验收

`crates/mode-protocol-client` 单元测试（9 条）：

- 请求/响应往返；
- 服务端错误映射；
- 通知与服务端请求在同一连接上多路复用（handler 应答）；
- 帧跨 chunk 拆分、U+2028/U+2029 不切帧；
- 单请求超时只失败该请求且 pending 清空；
- 对端 EOF 将 pending 失败为 `Closed`；
- 无 handler 的服务端请求回 `-32601`；
- 在飞期间 `idle_duration` 持续增长，pending 归零后重新计时。
