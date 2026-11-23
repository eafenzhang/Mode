# Spec：scoped 服务面登记表（LT-16.4d 底座）

状态：**已落地（2026-11-23）**——`rpc_host::ScopedServerRegistry`，7 条契约测试。

## 1. 为什么需要它

一次 remote workspace 挂接需要**独立的服务面**：renderer 那一侧用
`[0xFF][scopeId][payload]` 前缀区分，Host 这侧就要为每个 scope 起一个独立
`ChannelServer`，并把它出站的帧加同样的前缀。

此前壳内只有 `LocalScopeRegistry`——一张 `HashSet<u32>` 的「哪些 scope 处于挂接态」
表，它回答的是「这个 scope 能不能产生前缀帧」，**不能**承载「为某个 scope 注册一整套
频道」。`remote` 频道的 `attach`/`detach` 因此只能显式报错。

## 2. 与 TS 的对应

| TS（`scopedAttachments.ts`） | Rust |
| --- | --- |
| `attach(register)` → `ScopedAttachment` | `attach(register: FnOnce(&mut ChannelServer) -> Result<(), E>)` |
| `start()` | `ScopedAttachment::start()` / `registry.start(scope_id)` |
| `dispose()` | `ScopedAttachment::dispose()` |
| `detach(scopeId)` | `registry.detach(scope_id)` |
| `onDidDetach` | `registry.subscribe_detach(listener)` |
| `disposeAll()` | `registry.dispose_all()` |
| `count()` | `registry.count()` |

## 3. 三条时序硬要求

### 3.1 `attach` 不 ready

`ChannelServer` 以 `defer_init = true` 构造。原因：页面端口尚未注册时发 `Initialize`
会被解复用按**未知 scope** 丢弃且不重发，客户端永久停在 Uninitialized——所有 RPC
无报错无日志地排队（这条坑在 RPC 客户端那边已经踩过一次，见
`rpc-client-and-lan-transport.md` §2.1）。所以流程是「先建、后 `start`」。

### 3.2 `register` 失败要回收

`register` 返回错误时必须把 scope 收回（`count()` 减回去、转发器 abort）。
否则 scope id 被占着，业务层的 attachment 表也会对不上。

### 3.3 `detach` 的顺序

```
从表里摘掉 → 停转发器 → dispose 服务端 → 通知 onDidDetach
```

反过来（先 dispose 再停转发）会让 dispose 期间的帧写进一个已经没人读的通道，
或者让 detach 之后仍有帧漏出去。

## 4. 帧进出

- **出**：每个 scope 一条独立 `mpsc`；转发器收到 RPC payload 后
  `encode_scoped_frame(scope_id, payload)` 再交给 `ScopedFrameSink`。
  scope id 恒 ≥ 1，编码失败只可能是 bug，因此打 `eprintln` 而不是静默吞。
- **入**：`handle_frame` 要求帧**已带** scope 前缀；base 帧与未知 scope 都返回
  `false`（调用方按未知帧丢弃，与 TS 解复用同）。

## 5. 与 TS 的差异（登记）

1. **`attach` 的 `register` 返回 `Result`**：TS 用异常，Rust 用 `Result<(), E>`。
   语义等价（都由 `try/catch` 映射成「回收 + 上抛」）。
2. **日志走注入的 `ScopeLogSink`**：TS 直接调 `options.log`。壳内日志要按
   `AGENTS.md` 走结构化 sink，所以做成注入。
3. **`disposeAll` 会清空 detach 监听者**：TS 是 `detachEmitter.dispose()`。
   语义等价（此后不再有任何回调）。

## 6. 验收

`cargo test -p rpc-host --test scoped_registry`（7 条）：

- `attach` 阶段**一帧都不发**；`start()` 之后才发 `Initialize`（且带正确 scope 前缀）；
- `register` 失败后 `count()` 归零、无悬空 id；
- 两个 scope 各自路由，回帧前缀回到**发问的那个** scope；
- base 帧与未知 scope 都返回 `false`；
- `detach` 停转发 + 通知监听者 + 幂等（重复 detach 不重复通知）；
- `disposeAll` 清空全部 scope 与监听者；
- `start` 未知 scope 返回 `false` 并留痕 `scope_ready_unknown`。

全部用**真 `ChannelServer`**：帧真的编码/解码过，转发器真的跑在 tokio 上。
