# Spec：RPC 客户端与 LAN transport（LT-16.4d 续段）

状态：**首切落地（2026-11-23）**——`rpc-client` 的 `ChannelClient` 已进 Rust 并有真服务端对拍；
LAN / SSH / WSL / Docker / v3 的接线尚未落地。

## 1. 为什么先做客户端

16.4d 的剩下部分全是 transport，但四个 transport 卡点各不相同：

| transport | 卡点 | 需要什么 |
| --- | --- | --- |
| LAN | 无 | 客户端 + WS |
| SSH / WSL / Docker | 无 Rust SSH 客户端 | 客户端 + `russh` |
| v3 | 无 Noise 实现 | 客户端 + `snow` |

三者**共同的缺口是同一个**：Rust 侧只有 `rpc_host::ChannelServer`（服务端），没有
`ChannelClient`（客户端）。TS 侧 `connectViaProtocol(new SocketProtocol(socket))` 那一层
在 Rust 里不存在，所以任何一种 transport 都接不上。

因此先补客户端：它一旦存在，LAN 只剩「HTTP 取 capability + WS 建连」，
SSH 只剩「拉起 stdio」，v3 只剩「套 Noise」——三者都退化成插一个 transport。

## 2. `ChannelClient` 语义（对齐 `packages/rpc/src/channelClient.ts`）

### 2.1 初始化门

服务端在**建连瞬间**发一帧 `Initialize`，且常与握手回包同批到达。客户端必须：

1. 允许在 `Initialize` 之前就发起请求——**排队而不是丢弃**；
2. 收到 `Initialize` 后按入队顺序补发；
3. **重连场景**：第二次收到 `Initialize` 时，把已有的事件订阅按**原 id 重放**
   （新服务端不知道旧订阅）。

TS 那段关键注释（`lanRemoteAttach.ts`）说明「必须同步构造」的正是这个：
如果等 `open` 之后才建协议层，`Initialize` 会被静默丢弃且不重发，客户端永久停在
Uninitialized，所有 RPC 无报错无日志地排队。

### 2.2 请求与事件

| 方向 | header | id 分配 |
| --- | --- | --- |
| `call` | `[100 /*Promise*/, id, channel, method]` | 单调递增，从 0 起 |
| `cancel` | `[101 /*PromiseCancel*/, id]` | 复用原 id |
| `listen` | `[102 /*EventListen*/, id, channel, event]` | 与 call 共用计数器 |
| `dispose` | `[103 /*EventDispose*/, id]` | 复用原 id |

响应按 id 分发：`201 PromiseSuccess` / `202 PromiseError` / `203 PromiseErrorObj` /
`204 EventFire`。`200 Initialize` 无 id。

### 2.3 事件订阅是**惰性**的

TS 在「首个监听者加入」时才发送 `EventListen`。Rust 侧沿用：`listen()` 注册即发送
（没有 TS 的 Emitter 惰性语义），但**取消订阅必须发 `EventDispose`**，否则服务端的
订阅永远不会释放。

### 2.4 dispose 的 fail-closed

`dispose()` 必须 reject **所有**在途请求，包括还排在 Initialize 之前的那些。
TS 注释说明了后果：不这么做，上层的 in-flight 去重 Promise 会永久占用 workspace key。

## 3. 与 TS 的差异（登记）

1. **`PromiseError` 的 body 解析**：TS 构造 `Error` 并把 `code/kind/status/...` 等
   透传键挂到 error 上；Rust 侧返回 `ChannelError::Error`（已有结构），透传键保留在
   `extra` 里。语义等价，形状不同。
2. **惰性订阅**：Rust 的 `listen()` 立即发送 `EventListen`（TS 要等第一个监听者）。
   对服务端而言只是早发一次；不影响正确性。
3. **重连重放**：Rust 侧在第二个 `Initialize` 到达时重放已登记的订阅，与 TS 同；但
   Rust 不做 `notifyTransportClosed` 的「保留订阅、拒绝在途」二段式——transport 断开
   由调用方处置（transport 归 transport 层）。

## 4. 未落部分

- LAN transport（`POST /api/rpc-host-capability` + `GET /ws/host` + 帧桥）未实现；
- SSH / WSL / Docker 与 v3 未开始；
- 因此 `remote` 频道仍未进 `service_channel_names()` 与 inprocess 默认集。

## 5. 验收

- `cargo test -p rpc-client`：与**真 `rpc_host::ChannelServer`** 端到端对拍——
  Initialize 门、排队请求补发、事件订阅与释放、PromiseError 形状、dispose 的
  fail-closed、重连重放订阅。
- 全套门禁：clippy -D warnings、typecheck、lint、architecture。
