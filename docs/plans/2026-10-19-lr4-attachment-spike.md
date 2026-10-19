# LR-4 · DR-LR-2 附件传输 spike（单端口多路复用）

状态：2026-10-19 完成 spike（可执行验证通过）；生产接线未开始。
前置：迁移计划 `2026-10-19-lan-remote-sidecar-migration.md` §5.4 / DR-LR-2；
Host 契约 `docs/specs/tauri-renderer-host.md` §21。

## 1. 问题

`connectRemote` 成功之后，每个 remote logical session 需要一个独立的 service scope
（文件/Git/对话/任务都打该 scope 的 services）。Electron 线用
`MessageChannelMain` 每条 session 建一个 port，经 `InternalChannels.ScopedServicePort`
投给 renderer；Tauri 桥目前只有**一个**页面内 `MessageChannel`：

- `crates/sidecar-host/src/bridge.ts` 在页面里 `new MessageChannel()`，`port1` 交给
  renderer，`port2` 经 `host_send` / `host_recv` 与壳的 stdin/stdout 对接；
- `packages/desktop/src/host-standalone/index.ts` 只有**一个** `ChannelServer`
  跑在这条 stdio 帧流上；
- Tauri/wry **没有**从 Rust 侧创建/转移 `MessagePort` 的能力（M3 Task 0 spike §4 已实证），
  所以「让壳多开几个 port」这条路不存在。

## 2. 两个候选

### A. 单端口多路复用（推荐）

在页面桥与 Host 之间给帧加一个 **scope 前缀**；Rust 壳继续只搬字节，不需要知道 scope。
Host 侧按 scope 把帧派发给多个 `ChannelServer`，页面侧按 scope 派发给多个
`ChannelClient`。

### B. 页面侧多端口 broker

页面为每个 attachment 建一个 `MessageChannel`，把各自的 `port2` 都桥回壳。
问题：壳到 Host 只有**一条** stdin/stdout 管道；多个 port 到壳之后仍然要在壳里
按 ownership 分流并写进同一个 stdin，等于在 Rust 里再做一遍 A 的解复用，还要新增
端口生命周期命令。**没有减少任何改动，只是把多路复用挪进 Rust**。
而且 Tauri 侧无法把多个 port 作为一组稳定地投递给页面（每个 port 仍要页面自己建）。
结论：B 不成立，除非未来壳提供真正的多路 IPC 通道；当前不选。

## 3. 推荐协议

```text
base scope（本地窗口 Host）:   [ 原帧字节 ]                       ← 完全不变
remote scope:                  [0xFF][scopeId: u32 BE][原帧字节]
```

- `0xFF` 是保留首字节：`RequestType` 从 100 起、`ResponseType` 从 200 起，0xFF 与现有
  帧类型不冲突；**base 帧逐字节不变**，现有 stdio/端口往返用例不需要改。
- `scopeId` 由 **Host 分配**（`remote.attach` 的响应），renderer 只能使用 Host 发下来的 id；
  页面伪造的 id 在 Host 侧没有对应 `ChannelServer`，直接丢弃并计数。
- 帧长上限不变（RPC 层自己的限制）；前缀只增加 5 字节，不改变分帧边界。
- 流控：stdio 路径本来没有 `MessagePortProtocol` 的 flow-control 对象；scoped 协议同样
  只承载二进制帧，不引入新的流控语义。

## 4. 生命周期

```text
attach:
  renderer ── remote.attach(sessionId, scope) ──► Host
  Host: 在 windowRemoteConnectionRegistry 上建 scoped ServiceCollection + ChannelServer
  Host ── { scopeId } ──► renderer
  renderer ── bridge.createScopedServicePort(scopeId) ──► 页面 MessageChannel
  renderer: createMessagePortServiceConnection(port) → 注册到 workspace session

detach / session close:
  renderer 释放 ChannelClient + port；bridge 撤掉 scope 路由
  Host 释放 ChannelServer + scoped services；之后该 id 的帧计入 dropped

Host 重启：
  所有 scopeId 失效；renderer 必须重新 connect/attach，禁止继续复用旧 attachment
```

`scopeId` 只分配一次、不复用。renderer reload 时页面桥重新建端口，Host 侧旧 attachment
由 lease/断连清理（沿用 `windowRemoteConnectionRegistry` 的 owner/lease 语义）。

## 5. 失败模式

| 场景 | 行为 |
| --- | --- |
| 未知 scopeId 的帧 | 丢弃并计数（不误投 base） |
| attach 前收到 scoped 帧 | 丢弃；同一 id 之后 attach 也不补发旧帧 |
| 重复 attach 同一 id | Host 拒绝（分配新 id，不复用） |
| detach 时仍有在途 RPC | scoped `ChannelServer`/`ChannelClient` dispose，挂起请求以 disposed 失败 |
| 大帧（256KB） | 前缀不影响序列化；spike 已覆盖 |
| Host 崩溃/退出 | 通道断开，renderer 走 session 降级；不得继续路由到失效 scope |
| 页面重载 | 旧端口随页面销毁；Hidden Host 的 attachment 由 lease 清理，新页面重新 attach |

## 6. 可执行验证（已通过）

`packages/desktop/tests/attachment-multiplexer-spike.test.mts` 内联了原型（不改生产传输层）：

- 同一根传输上并发 base + scope 1 + scope 2，三个 `who` 响应分别返回自己的 label；
- scope 1 发 256KB 大帧往返成功，base 随后仍可用；
- 关闭 scope 1 后：未知 scope 帧被计入 `droppedScopes`、base 与 scope 2 继续工作、
  scope 1 的调用以 disposed 失败收口。

```text
✔ base 与两个 remote scope 并发复用一根传输，响应不串台
✔ scope 回收只影响自己；未知 scope 的帧被丢弃
```

## 7. 生产化步骤（LR-4 实施）

状态（2026-10-19）：1、2、3、6 已落地；4 的 renderer 消息形状本来就兼容（页面桥现在按它投递），
生产 attach 入口（5）未接。

1. ✅ `packages/rpc/src/frameMultiplexer.ts`：marker 编解码、demux、`createScopedProtocol`、
   `scope-ready/scope-detach` 控制帧；单测覆盖 marker/非 marker、未知 id、畸形帧、大帧、重复 attach。
2. ✅ `packages/desktop/src/host-standalone/index.ts`：stdin 先处理控制帧，再解复用 RPC 帧；
   base 仍进现有 `ChannelServer`，scoped 帧进按 id 注册的 server；outbound 由各 scoped protocol
   加前缀。测试入口 `MODE_HOST_TEST_SCOPES=1` 提供 `attachTestScope`（生产由 `remote.attach` 接）。
3. ✅ `crates/sidecar-host/src/bridge.ts`：`createScopedServicePort` / `releaseScopedServicePort`；
   入方向按 marker 分流；renderer 的 `ScopedServicePortReady` 被翻译成 `scope-ready` 控制帧。
   Rust 壳零改动。
4. ⏳ `packages/desktop/src/renderer/src/main.tsx`：已有的 `parseRemoteWorkspaceServicePortMessage`
   + `registerRemoteWorkspaceSession` 直接消费页面桥投递的 scoped port；生产 attach 的 session
   元数据仍待 `remote.attach` 提供。
5. ⏳ Host 侧 `remote.attach` / `remote.detach`：分配/回收 scopeId，接
   `windowRemoteConnectionRegistry` 与 scoped `ServiceCollection`。
6. ✅ 端口往返用例：base 用例零改动；新增两个 remote scope 并发、detach 回收、
   未知 scope 丢弃、bridge 前缀与 ready ACK。

**已落地的握手顺序**（避免 Initialize 被解复用按未知 scope 丢弃）：

```text
Host attach 返回 scopeId（scoped ChannelServer 已建、尚未 ready）
renderer 调 bridge.createScopedServicePort({scopeId, attachment 元数据})
bridge 投递 mode:scoped-service-port（端口 transfer）→ renderer 建 ChannelClient
renderer 注册完成后 post mode:scoped-service-port-ready（带 attachmentId）
bridge 发控制帧 {type:"scope-ready", scopeId} → Host 对该 scope 调 server.ready()
detach：renderer 释放端口 → bridge 发 {type:"scope-detach"} → Host 回收 scope
```

## 8. 开放项

- **scopeId 与 workspaceIdentity 的绑定校验**：attach 必须同时校验
  `remoteSessionId + workspacePath + workspaceIdentity` 三元组，不能只按路径匹配。
- **desktop-continuous 与 web-remote-replayable**：多路复用只改传输，不改变两种语义；
  手机远控仍走 Host attachment，不新建第二条链路。
- **流控/背压**：stdio 路径无 flow control；大文件操作沿用 RPC 层现有行为。
  若未来需要，先在 `frameMultiplexer` 上加控制帧，不改 base 帧格式。
- **安全**：`remote.attach` 只在 local-only 管理面；LAN 对端不能调用。
