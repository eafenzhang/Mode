# Spec：`remote` workspaces 迁移（LT-16.4d）

状态：**首切落地（2026-11-23）**——registry 状态机 + SSH host key 已进 Rust；RPC 频道与
四种 transport 未迁。

## 1. 面拆解

`IRemoteService`（`packages/services/src/remote/remoteService.ts`）9 个成员，实现分布在
两层，**能迁与不能迁的分界线很清晰**：

| 层 | 文件 | 规模 | 性质 |
| --- | --- | --- | --- |
| registry 状态机 | `packages/desktop/src/host/windowRemoteConnectionRegistry.ts` | 754 行 | **传输无关**：session 生命周期、按 key 复用连接、generation、取消语义 |
| 服务适配 | `packages/desktop/src/host-standalone/remoteWorkspaceService.ts` | 917 行 | 混合：事件发射、attachment、Bot 桥、controller source |
| transport | `lanRemoteAttach` / `remoteV3/*` / SSH·WSL·Docker stdio | — | **不可首切**，见 §2 |

## 2. 为什么 transport 不能一起迁

| transport | 阻塞点 |
| --- | --- |
| LAN | WS 客户端 Rust 有 `tokio-tungstenite`；但被控端服务集合暴露、scoped 频道守卫都在 Node |
| SSH / WSL / Docker | **Rust 侧无 SSH 客户端**（`cargo` 全仓 grep `ssh2`/`russh`/`openssh` 为 0），且要在对端部署资源包 |
| v3 | 传输是 **Noise E2EE**，Rust 侧无 Noise 实现（grep `noise`/`snow` 为 0） |

这三条都不是「写个频道」的量级。因此首切取 registry：它是三种 transport 的**公共前缀**，
迁完即可让后续每种 transport 只需实现一个 connect 端口。

## 3. 已落：`crates/remote-session-registry`

### 3.1 连接复用键（`build_connection_key`）

与 TS 逐字节一致，**这是最容易写错的地方**：

| kind | key |
| --- | --- |
| ssh | `ssh:` + `build_ssh_remote_host_key(target)` |
| wsl | `wsl:<distro\|"\<default>">\0<user\|"\<default>">`（**含 `\0` 分隔符**） |
| docker | `docker:dedicated:<remoteSessionId>`（**不按 target 复用**） |
| lan | `lan:<host 小写>:<port>` |
| v3 | `v3:<deviceId>` |

docker 刻意不复用：TS 原注释「保持现有 dedicated logical session 生命周期」。

`build_ssh_remote_host_key` 落在 `workspace-identity`（它已有 `RemoteTargetRef`），
输出 `JSON.stringify(["ssh:v1", host, port, username, authKind, privateKeyPath])`
——**authKind 与归一后的私钥路径参与身份**，密码本身不进 key。

### 3.2 session 状态机

```
connect(requestId, target, path?, identity?)
  ├─ requestId 重复 → Err（TS 抛错，不静默）
  ├─ 同 key 有 connecting/online entry → 复用（清 idle 定时器）
  └─ 否则建 entry → state=connecting
        └─ transport 完成后 → 所有未取消 session 置 online
cancelConnect(requestId)
  ├─ 未找到/已取消 → 幂等返回
  ├─ session 标记 cancelled + 从两张表移除
  └─ entry 无剩余 session 时按 kind 分支处理（见 3.3）
bindContext(sessionId, path, identity?)
  ├─ 未找到 → Err
  ├─ generation += 1（每次换绑都加，快照据此判断新鲜度）
  └─ workspaceKey 变化时才释放旧 workspace
dispose(sessionId)
  └─ 释放 session 与其 workspace 归属
```

`generation` 语义：connect 时为 1，**每次 bindContext 加 1**。它是 renderer 判断
「这份快照是否还是我绑的那次」的唯一依据。

### 3.3 最后一个 waiter 取消时的分支（TS 的核心防御）

取消后 entry 没有剩余 session 时，**不能一律销毁**：

| 条件 | 动作 | 原因 |
| --- | --- | --- |
| ssh 且 state=connecting | 按**对象身份**退休旧 entry 再 abort | 旧 entry 仍以 connecting 留在复用表；立即重连会等一个已 abort 的 readiness 并沿用旧凭据。迟到完成不能删/复活同 key 的新连接 |
| wsl 且 state=online | 排 idle 销毁（默认 60s） | WSL 启动贵，短时间复用 |
| ssh 且 state=online | 只清 idle 定时器 | ready 的 SSH 连接是窗口 cache，取消一次 attach 不该炸掉后续复用 |
| 其余且有 handle | 立即销毁 | — |

「迟到完成」的守卫体现在 `settle` 里：entry 已 disposed 或 session 数归零时，
**完成结果必须立刻释放，不能复活旧连接**。

## 4. 未迁与降级语义

- **RPC 频道 `remote` 未落地**：registry 目前只有库 API，没有 `ServerChannel` 包装，
  所以**不进 `service_channel_names()`、不进 inprocess 默认集**。
- **`attach`/`detach` 未迁**：依赖 scoped service 集合 + `MessagePort`，壳内
  `rpc-host::LocalScopeRegistry` 只有 scope 挂接表，没有「把另一套服务集合注册进去」的
  能力。这一半必须等 transport 与 service-collection 暴露一起做。
- **`onDynamicConnectionEvent` 未接**：registry 已产出事件值（`RemoteWorkspaceEvent`），
  但发射器与 RPC 订阅绑定在频道层。

## 5. 验收

- `cargo test -p remote-session-registry`：连接键五种 kind、docker 不复用、requestId
  重复拒绝、取消幂等、generation 递增、迟到完成不复活、bindContext 换 workspace 释放旧归属。
- `cargo test -p workspace-identity`：`build_ssh_remote_host_key` 的 authKind 优先级
  （privateKey > password > agent）与私钥路径归一（盘符大小写、`..` 越界、UNC）。
- 全套门禁：clippy -D warnings、typecheck、lint、architecture。
