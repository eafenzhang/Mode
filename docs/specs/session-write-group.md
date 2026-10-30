# 会话写路径组（第一批：close 与三个 setter）

> 范围：`crates/agent-protocol::session_write` 与 `crates/agent-process` 的 facade 方法。
> 对应 TS `modeAgentService` 的 `closeSession` / `setModel` / `setThoughtLevel` / `setMode`。

## 1. 方法

| 方法 | 协议方法 | params | result |
| --- | --- | --- | --- |
| `close_session` | `session/close` | `{sessionId, expectedPersistence?}` | `{closed?}`（缺省按成功 `true`） |
| `set_session_model` | `session/setModel` | `{sessionId, model, expectedRevision?, persistAsWorkspaceLastUsed?}` | 完整状态快照 |
| `set_session_thought_level` | `session/setThoughtLevel` | `{sessionId, thoughtLevel?, expectedRevision?, persistAsWorkspaceLastUsed?}` | 完整状态快照 |
| `set_session_mode` | `session/setMode` | `{sessionId, mode, expectedRevision?}` | 完整状态快照 |

- `persistAsWorkspaceLastUsed` 在 schema 里是 `z.boolean().default(true)`，但请求侧语义是
  「undefined 省略」：Rust 镜像用 `Option<bool>`，缺省时不写字段。
- `thoughtLevel` 缺省同样省略键。
- 三个 setter 的结果复用 `agent_protocol::snapshot::SessionStateSnapshot` 严格解析。

## 2. 与 TS 的差异

TS 的 `getClient` 会先读 model-selection readiness，未就绪时抛 `provider_not_ready` 且不拉起
Agent。本 facade 只做 `get_or_spawn`；readiness 门禁属于服务层编排，接入频道时补齐。

## 3. 验收

1. 共享夹具覆盖四个请求与四个应答：`request/response-session-close`、
   `request/response-session-set-model`、`request/response-session-set-thought-level`、
   `request/response-session-set-mode`；TS zod 与 Rust 镜像同时接受，Rust 序列化回到线上字节。
2. `expectedRevision` / `persistAsWorkspaceLastUsed` / `thoughtLevel` 缺省时不出键。
3. `closed` 缺省时返回 `true`。
4. 非法结果分别报 `invalid session/close result` 与 `invalid session/<setter> result`。
