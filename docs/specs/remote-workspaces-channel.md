# Spec：`remote` workspaces 迁移（LT-16.4d）

状态：**频道 + LAN 端口 + 分派器均已落地，仍未翻面（2026-11-23）**——registry 状态机、SSH host key、RPC 客户端、频道包装、scoped 服务面、attach/detach、LAN 端口、transport 分派器均已进 Rust；SSH/WSL/Docker 与 v3 的端口实现未做。

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

「迟到完成」的守卫体现在 `complete_connect` 里：entry 已 disposed/aborted 或
session 数归零时，**完成结果必须立刻释放，不能复活旧连接**。

### 3.4 workspace 运行时归属（只有 WSL 有）

TS 把「同一个 workspace 的运行时」与「连接」分开记账，Rust 侧原样搬了这一层：

```text
entry.workspace_runtime[workspaceKey] = {
  context:              { workspacePath, workspaceIdentity? }   # 释放时回传给调用方
  generation:           u32                                     # 归属代际，不是连接代际
  pending_release_generation: Option<u32>                       # 因 running task 推迟
  release_in_flight:    bool                                    # 是否有一次释放在进行
}
```

三条不变量：

1. **代际只在「没有别的 owner」时 +1**。上一代 release 还在飞（等 running task 归零）时
   新 owner 抢到同一 workspace，代际必须递增，否则旧 release 完成后会把**新 owner 的**
   runtime 删掉。
2. **running task > 0 时推迟释放**，记下 `pending_release_generation`；归零后再补做。
   归零补做时必须校验代际——旧代际的待办要失效。
3. **释放必须成对**：`ReleaseWorkspace` 发出后调用方要回传
   `complete_workspace_release(entry_id, workspace_key, generation)`，否则
   `release_in_flight` 永远为真，同 workspace 的下一代 owner 会一直停在 `Pending`。

`requestWorkspaceRelease` 的四种「不释放」情形原样保留：非 WSL、缺 key/代际、
还有别的 owner、running task > 0。少一条就会把别人还在用的 runtime 拆掉。

### 3.5 `find_session_for_workspace` 的歧义处理

多个候选（例如两个 docker session 指向同一 workspace）时**报错而不是挑一个**。
静默选择会把消息发到错误的会话；TS 同（抛错）。

## 4. RPC 频道（已落，未翻面）

`host-channels::remote`（`REMOTE_CHANNEL = "remote"`）把状态机包成 `ServerChannel`：

| 方法 | 语义 |
| --- | --- |
| `connect` | 派生 identity → `begin_connect` → 起端口传输 → `complete_connect`；返回 `{ok:true,descriptor}` 或 `{ok:false,error}` |
| `cancelConnect` | 转发 `cancel_connect`（未知 requestId 幂等） |
| `bindContext` | 转发 `bind_context`；空 `workspacePath` 显式报错 |
| `dispose` | 转发 `dispose_session` |
| `getSession` / `listSessions` | 快照（`state`/`sourceAvailability`/`generation`） |
| `attach` | 为 session 建 scoped 服务面，返回 `{scopeId, attachmentId, sessionId}` |
| `detach` | 回收指定 attachment（未知 id 幂等） |
| `onDynamicConnectionEvent` | 只发当前订阅者，**不重放历史** |

### 4.1 `attach` / `detach`

`attach` 的四条约束：

1. **必须先 `bindContext`**。`workspacePath` / `workspaceIdentity` 缺失时报
   「先 bindContext 再 attach」——指向正确的修复动作，而不是给个空服务面。
2. **建 scope 但不 ready**。页面端口注册完 `ChannelClient` 后由宿主发
   `scope-ready` 控制帧 → `registry.start(scopeId)`。这条时序由
   `ScopedServerRegistry` 保证（见 `scoped-server-registry.md` §3.1）。
3. **服务面由工厂提供**（`RemoteScopeFactory::register`）。频道本身不假设
   服务集合内容——本地与远端集合都能挂。
4. **注册失败要回收**：scope 与 attachment 都不留。

`detach` 只回收**指定的**那一个 attachment（多个 attach 各自独立 scope）；
`dispose` 则回收该 session 的**全部** attachment（TS `detachSessionAttachments` 同序：
先摘 attachment，再释放 session）。

**未装配 scoped 服务面时**（`scopes`/`scope_factory` 为 `None`），`attach` 仍显式报错
而不是返回假 `scopeId`——假 id 会让 renderer 以为挂接成功，后续帧全部丢弃。

### 4.2 与 TS 的差异（登记）

**频道守卫的位置**：TS 在 `registerChannel` 入口包一层 `guardedServer`，一次覆盖全部
服务。Rust 的 `ChannelServer` 没有可拦截的注册入口，因此守卫由**工厂实现**在注册时
自行套用（例如包一层带校验的 `ServerChannel` 再 `register_channel`）。语义要求相同：
未知事件名不得让服务端 panic。

四条与 TS 对齐的关键取舍：

1. **`connect` 失败不抛 RPC 错误**，而是返回 `{ok:false,error}`。renderer 按 `ok`
   分支处理；抛错会走成未捕获异常。传输端口缺失同此。
2. **secret 一律不下发**：`descriptor.target` 里没有 `password` / `token` /
   `privateKeyPassphrase`。SSH 只保留 `privateKeyPath`（它参与共享键，属于身份而非
   秘密）。
3. **`attach`/`detach` 见 §4.1**：建 scope 但不 ready、服务面由工厂提供、
   dispose 回收该 session 的全部 attachment。

## 5. LAN transport 端口（已落）

`host_channels::remote_lan::LanTransportPort` 把 `rpc_client::lan` 接到
`RemoteTransportPort`。它只做三件事：**按 entry_id 记账连接**、委派建连、
在 abort/dispose 时断开。

四条容易写错的地方：

1. **令牌从凭据取，不从 target 抄**。`target.token` 只在连接流程内透明传递
   （TS `stripRemoteTargetSecrets` 同语义）；落盘与复用一律走凭据服务。
   为此 `RemoteTargetSpec::Lan` 增加了 `server_id` / `token` 两个字段——但两者
   **都不进 identity、不进连接键、不进任何下发描述符**（有断言）。
2. **没有配对就没有令牌**：建连**前**直接报「这台设备还没有配对」。先建 WS 再失败
   会在对端留下一次失败鉴权（测试断言 `issued == 0 && upgraded == 0`）。
3. **abort 与 dispose 都可能命中同一 entry**：map 先摘再断，避免重复 close。
4. **`entries` 用 `Arc<Mutex<…>>`**：`connect` 的 async 块是 `'static`，借不到 `&self`。
   端口 `Drop` 时断开全部连接，避免 Host 退出后 WS 悬挂。

`release_workspace` 对 LAN 是 **no-op**：对端是「已经在跑的 Mode 实例」，不部署也不
启动任何东西，没有 workspace 运行时需要释放（TS 同：只有 wsl 有 release）。

## 6. transport 分派器（已落）

`host_channels::remote_transport::RemoteTransportRouter` 按 **target kind** 把
`RemoteTransportPort` 的调用路由到具体端口。

为什么需要它：`RemoteChannel` 只认一个 `RemoteTransportPort`，而目标有五种 kind，
阻塞点各不相同。没有分派器时只能二选一——要么被迫给所有 kind 共用同一个端口（LAN
的端口会拿到 SSH 目标），要么翻面后所有非 LAN 目标都落到同一条含糊错误上。

三条记账要求：

1. **`entry_id` → kind 必须记下来**。`abort`/`dispose`/`release_workspace` 只拿到
   entry id；不能靠 target 反推——session 生命周期里 target 未必在手边。
2. **未注册 kind 的错误要准确**，说清「这种传输还没落地」而不是「远程服务未就绪」
   ——后者会让用户以为是配对或网络问题。例如：
   `SSH 远程工作区尚未支持：需要 SSH 客户端（未落地）`。
3. **失败与回收都不留悬空记录**：`connect` 失败不登记 entry（否则 `dispose` 会去断
   一个不存在的连接）；`abort`/`dispose` 先摘记录再转发，重复调用不会重复断开。

## 7. 未迁与降级语义

- **SSH/WSL/Docker 与 v3 的端口**：分别需 `russh` 一类 SSH 客户端与 `snow` 一类
  Noise 实现。LAN 已完成。
- **频道仍未进 `service_channel_names()` 与 inprocess 默认集**：整频道切换要求
  「所有 transport 都有降级路径」，而当前只有 LAN 一种端口。翻面后 SSH/v3 目标会
  返回 `{ok:false}`（可读错误，不是崩溃），是否可接受属产品决策，未擅自决定。

## 8. 验收

- `cargo test -p remote-session-registry`（20 条）：连接键五种 kind、docker 不复用、
  requestId 重复拒绝、取消幂等、generation 递增、迟到完成不复活（含 fail 之后）、
  WSL workspace 代际与释放成对、running task 推迟与补做、旧代际待办失效、
  `findSessionForWorkspace` 歧义报错、stats 分别计数。
- `cargo test -p workspace-identity`（10 条）：`build_ssh_remote_host_key` 的 authKind
  优先级（privateKey > password > agent）与私钥路径归一（盘符大小写、`..` 越界、UNC）。
- `cargo test -p host-channels --test remote_channel`（16 条）：connect 三种结局、
  secret 不下发、attach 未绑定上下文报错、attach 建 scope 但不 ready、detach 只回收
  指定 attachment、dispose 回收该 session 全部 attachment、工厂注册失败不留残留。
- `cargo test -p host-channels --test remote_lan_port`（4 条）：未配对在建连前失败
  （不换票、不升级 WS）、真 axum 服务的 connect→attach→dispose 全链路、LAN 的
  `release_workspace` no-op、端口 drop 不 panic。
- `cargo test -p host-channels --test remote_transport_router`（7 条）：按 kind 路由、
  未注册 kind 报准确错误、失败连接不留 entry、abort/dispose 按记录路由、未知 entry
  的 abort/dispose/release 都不打到端口、release 只打到归属端口、重复 dispose 不泄漏。
- `cargo test -p rpc-host --test scoped_registry`（7 条）：见 `scoped-server-registry.md`。
- 全套门禁：clippy -D warnings、typecheck、lint、architecture。
