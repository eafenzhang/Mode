# Spec：壳内 RPC 服务端分派语义（`crates/rpc-host`）

状态：2026-10-29 建立（W0-3）。行为真源：`packages/rpc/src/channelServer.ts`；
线格式见 `docs/specs/rpc-wire-format.md`。本 spec 描述**分派与所有权**，不描述传输。

## 为什么不复用 TS 实现

M4b 之后 renderer ↔ Host 的 RPC 由壳直接服务，Node `ChannelServer` 会退场。壳是 Rust，
所以需要一份与 TS 行为等价的契约；迁移期两者并存时，同一频道**同一时刻只允许一个所有者**，
由启动期的频道开关决定（见 `docs/plans/2026-10-29-m4b-host-into-shell.md` §M4b-3）。

## 输入与输出

- 输入：一条 RPC payload（已剥掉 stdio 的 4 字节长度前缀），由调用方从帧解码器取出。
- 输出：RPC payload，经 `mpsc` 交给调用方；是否加长度前缀、写到哪个通道由调用方决定。
- 非请求帧（回帧、无法解析的帧、未知请求类型）一律忽略，不产生回帧、不报错。

## 事件顺序

```
构造 ──► Initialize(200)                      （defer_init 时改为 ready() 触发）
Promise(100) ──┬─ 频道已注册 ──► 调用 ──┬─ 成功 ──► PromiseSuccess(201,id,data)
               │                        ├─ Error ──► PromiseError(202,id,{message,name,stack?,…})
               │                        └─ 其他值 ─► PromiseErrorObj(203,id,value)
               └─ 未注册 ──► pending 队列 ──┬─ 注册 ──► 立即补发（定时器取消）
                                            └─ 超时 ──► Unknown channel 日志
                                                        └ Promise 额外回 PromiseError
EventListen(102) ──► 订阅 ──► 每次 emit 一帧 EventFire(204,id,data)
PromiseCancel(101) / EventDispose(103) ──► 按 id 释放（取消令牌 / 订阅）
dispose() ──► 取消全部令牌 + 释放全部订阅 + 清空 pending
```

## 行为细则

1. **构造**：默认立即发 `Initialize`；`defer_init` 为真时由 `ready()` 发。
2. **错误映射**：JS `Error` 形态 → `PromiseError`，body 是
   `{message,name,stack?}` 加上实现方显式透传的键（对应 TS 的 `code/kind/status/
   retryAfterMs/data/detail/details/taskId/traceId`）；`stack` 为空时不写该键。
   非 Error 的拒绝值 → `PromiseErrorObj`，body 原样。
3. **未注册频道**：`Promise` 与 `EventListen` 都进 pending；频道注册后按注册顺序补发。
   超时（默认 1000 ms）时先打一行 `Unknown channel: <name>`；`Promise` 再回一条
   `PromiseError`，文案 `Channel name '<name>' timed out after <ms>ms`、`name` 为
   `Unknown channel`；`EventListen` 只留日志，不回帧。
4. **并发与顺序**：每个 `Promise` 独立执行，响应可以乱序到达，客户端按 `id` 匹配；
   **同一订阅**的 `EventFire` 按实现方 emit 的顺序发出（实现方负责顺序）。
5. **取消**：`PromiseCancel` 置位该请求的取消令牌；是否提前返回结果由频道实现决定，
   服务端不替它丢弃响应。
6. **释放**：`EventDispose` 调用订阅的释放函数；`dispose()` 释放全部订阅与取消令牌。

## 与 TS 实现的已知差异

- `PromiseError` 的 JSON 键序按字典序（TS 按写入序）。客户端只做 JSON 解析，键序无语义。
- `channel.listen` 抛错时 TS 会让异常冒泡（调用方可能崩），Rust 侧忽略并视为「没有订阅」，
  不伪造回帧。
- 超时日志固定写 stderr；TS 是 `console.error`，落点由宿主决定。

## 验收

`crates/rpc-host/tests/channel_server.rs` 覆盖：Initialize、成功回显、Error/ErrorObj 映射、
事件连发与释放、取消置位、pending 补发、pending 超时错误、dispose 释放、非请求帧忽略。
