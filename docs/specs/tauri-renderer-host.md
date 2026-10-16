# Spec：Tauri 壳的 Host 契约（M3 Task 1）

前置：阶段计划 `docs/plans/2026-10-16-tauri-m3-ecosystem.md`（DR-M3-1/2/3）、
任务子计划 `docs/plans/2026-10-16-m3-host-sidecar.md`（DR-T1-1/2/3）、
Task 0 spike `docs/plans/2026-10-16-m3-port-transport-spike.md`（§3/§4 是本契约的路线来源）、
M1 的挂载缺口表 `docs/specs/tauri-platform-commands.md` §4.2。
纪律：`docs/specs/clean-room-policy.md`——`crates/**` 与 `apps/desktop-tauri/**` 全部适用；
`packages/desktop/src/host-standalone/**` 是新写文件。

本文件在 2026-10-16 的复评后修订过一次（fix round 1）：C1（`listen` 抛错会杀死 Host）、
I2（`MODE_DATA_BASE_DIR` 语义）、I3（typecheck 覆盖）、I4（CI 接线与口径）、I5（框架无关拆分的理由）
与若干 Minor 都已落进下面的正文；改动痕迹见 §10 的证据登记与 `task-1-report.md` 的 fix round 1 小节。

**2026-10-16 M3 Task 2 再修订（服务面换真）**：§0 的状态、§1 的所有权（`deferInit` 使「第一帧」= 服务面齐备）、
§2 的环境契约（新增三条）、§3.1（`listen` 危险的**守卫实现**）、§4（真服务**会写盘**之后的隔离口径）、
§5 的覆盖分工、§8 的边界与新增的 §11–§14（startup 序列与映射口径、原生依赖、打包形态与体积、
崩溃重启策略）都是本任务的产物。**渲染层仍未改动**。

## 0. 本任务做到哪、没做到哪（先读这条）

**做到**：Tauri 壳能拉起一个 Node **Host sidecar**，把渲染层要的两样东西送进 webview——
一个承载 `ChannelServer` 的真 `MessagePort`，以及一条 database-startup 状态流——
于是渲染层**未改动**的启动闸门会放行（`phase === "ready"` 且两端 `startupId` 同代）。

**Task 2 之后的状态（本文件当前描述的对象）**：服务面是**真的** `packages/services`（§11.1 的
启动序列，`createLocalServices` + `exposeOnChannelServer`），闸门由**真实启动**驱动——
入口用 `deferInit = true`，`server.ready()`（即壳看到的「第一帧」）**在真存储打开、真服务频道
全部注册之后**才发，所以「闸门放行」与「服务面齐备」在时序上是同一件事（§1 末段）。
失败路径同样是产品件：Host 启动失败 → 结构化落 stderr 并以非零码退出 → 壳的 reaper 推
`failed(transport_closed)`（§3.1、§10.4、§10.5）。

**仍然没做到（都不在 Task 2 的授权内，逐条登记）**：

- **真实 renderer 挂载**（Task 3 换 `frontendDist`）——本任务的全部真机证据用的是集成测试页
  （`test/harness/host-sidecar/**`），它不是渲染层；
- **Host 的分段阶段没有传到渲染层**：`preparing_host_storage` / `starting_services` 这些是
  Host 进程内的真事实（`host.describe().startup.history` 可观察、有用例钉住），但**没有**一条
  Host → 壳的通道把它们合并进壳发布的 `DatabaseStartupState` 流。渲染层看到的仍是
  `starting` → `ready`（或 `failed`）。理由：那条通道要动传输层，而本任务的约束是
  「不改壳/传输层」——**差额登记在这里**，Task 3/4 定 transport 时一并处理；
- **窗口模型、跨窗口广播、手机远控**（Task 3/4）：`createLocalServices` 的缺省注入下这些能力
  各自表现为「不可用/空操作」（`parentPort` 传 null 时广播为空操作），不是静默走另一条路径；
- **崩溃重启策略**：§14 明确登记为不做（含理由与代价）；
- **CUA 资产在打包形态里的落点**（Task 5）：探针只验证「注入了但不存在 = 失败」（§12）。

**渲染层的超时口径（复评 I4(b) 纠正）**：渲染层在**一条状态都没收到**时 30 秒后画失败屏
（`packages/desktop/src/renderer/src/main.tsx` 的启动计时器）；那个计时器**一收到任何状态就被拆除**。
所以一旦 `starting` 已经投递出去，渲染层就**没有自己的超时**了——**壳推的 `failed` 是此后唯一的
失败信号**。这也是 DR-T1-3 把「失败必须由壳主动推」定成硬要求的原因；
（本文件早期版本写成「40 秒」「超时是兜底」，两者都不准确，已改。）

**仍然未验**：真实 renderer 挂载（Task 3 换 `frontendDist`）、`node:sqlite`/`node-pty` 在
sidecar 里可用（Task 2）、打包形态的 sidecar 落点与体积（Task 3/5）。

## 1. 组件与所有权

| 组件 | 位置 | 职责 | 不做什么 |
| --- | --- | --- | --- |
| Rust 壳 | `apps/desktop-tauri/src-tauri`（`main.rs`、`host.rs`、`window.rs`） | 建窗口 + 注入桥；spawn/监督 Host；两个 raw 帧命令 + 一个状态长轮询；退出时收 Host | 不持业务状态（`AGENTS.md` 的「进程、协议与远程控制」） |
| Host sidecar | `packages/desktop/src/host-standalone/**`（Node，入口 `index.ts`） | 真 `ChannelServer` + **真服务面**（`createLocalServices`，40 个频道，§11.1）；数据根解析；帧形状校验；**三条守卫**（stdout 只走帧、`listen` 不抛、派发不炸）；真启动序列 | 不建窗口、不碰 Tauri；不实现窗口模型/远控（Task 3/4） |
| 进程/传输库 | `crates/sidecar-host/**` | spawn/监督/帧编解码/桥脚本/启动状态/路径挑选纯函数 | **不引用任何 Tauri 类型**（理由见 §6） |

**「第一帧」为什么等于「服务面齐备」（Task 2 换掉了合成 ready）**：入口用
`new ChannelServer(protocol, ctx, 1000, /* deferInit */ true)`，`server.ready()` 在
**真存储打开 + 全部服务频道注册**之后才调；而 `ready()` 就是那第一帧（`Initialize`）。
于是壳的就绪判据（§1 末段的所有权口径）不需要新增通道就自动变成了「真启动完成」——
真机实测的 `startup.history` 是 `["1:preparing_host_storage" ×9, "10:starting_services", "11:ready"]`
（§10.5）。这条顺序不靠人记得：`apps/desktop-tauri/test/realServices.test.mts` 有断言。

**startupId 的所有权**：壳生成、经环境变量 `MODE_HOST_STARTUP_ID` 交给 Host，并烘进注入桥。
「端口负载的 `databaseStartupId`」与「状态流的 `startupId`」由此天然同代——这正是闸门的判据。

**发布状态的所有权（Task 1 的口径，Task 2 不动）**：**壳是渲染层看到的 `DatabaseStartupState`
的唯一发布者**（`crates/sidecar-host/src/startup.rs` 的 `StartupMonitor`）。它合并两类事实：

- Host 进程事实（壳直接观测）：就绪判据是 **Host 的第一帧**（`ChannelServer` 的 `Initialize`）；
  失败判据是起不来 / 就绪前退出 / 就绪期限内没有第一帧 / 就绪后崩；
- **Host 报告的分段阶段（`preparing_host_storage` 等）本任务没有合并进来**：那需要一条
  Host → 壳 的通道（壳现在是「字节双向搬运」，没有解析 Host 语义的通道）。差额的**代价是
  真实的**：真机失败跑里 Host 自己报的是 `errorCode=sql_failed failedPhase=preparing_host_storage`，
  而渲染层收到的是壳能判断的 `transport_closed` / `failedPhase=starting`（§10.6 的原始输出）。
  「失败要能被推给渲染层」这条硬要求达成；「失败原因要精确到阶段」这条**未达成**，登记为
  Task 3/4 定 transport 时的输入。

为什么不让渲染层同时听两个发布者：闸门的判据是「同代匹配」，两个发布者会让「同代」的语义分裂。

## 2. 环境契约（壳 → Host）

| 变量 | 由谁设置 | 含义 |
| --- | --- | --- |
| `MODE_HOST_STARTUP_ID` | 壳显式设置 | 本次实例的启动代 id（`[A-Za-z0-9_-]`） |
| `MODE_DATA_BASE_DIR` | 壳显式设置 | 数据**基**目录（不是数据根；见 §4） |
| `HOME` / `USERPROFILE` | **继承**（壳不显式覆写） | 家目录：壳用它派生基目录，Host 用它兜底派生 |
| `MODE_HOST_SIDECAR_ENTRY` | 进程环境 → 壳 | 入口脚本显式路径（开发/测试用；打包形态走资源目录） |
| `MODE_HOST_NODE` | 进程环境 → 壳 | Node 可执行文件（默认 PATH 上的 `node`） |
| `MODE_HOST_SIDECAR_CWD` | 进程环境 → 壳 | Host 工作目录（`--import tsx` 的裸模块解析从它开始） |
| `MODE_HOST_NATIVE_DEP_PROBE` | 进程环境 → Host | 设为 `spawn` 时原生依赖探针**真起一个 pty**（测试用；产品默认不设，见 §12） |
| `MODE_BUILTIN_PROVIDER_CONFIG_FILE` | 进程环境 → Host | Mode Built-in Provider Config 的显式路径（第一优先，见 §12.2） |
| `MODE_HOST_RESOURCES_DIR` | 进程环境 → Host | 打包形态的资源目录（第二优先；里面的 `config/provider/mode-builtin.json`） |

`HOME`/`USERPROFILE` 一栏在 fix round 1 更正：壳**不**显式设置它们（早期版本的表写错。
隔离靠「测试自己在父进程环境里覆写这两条 + 壳把值透传给子进程」实现——子进程默认继承父环境）。
只设 `MODE_DATA_BASE_DIR` 不足以隔离（M2 的实测结论：版本化数据树走 `MODE_DATA_BASE_DIR`，
CLI 布局走 `HOME`），所以用例与本地跑一律**双覆写**。

入口解析的**挑选规则**（纯函数 `sidecar_host::pick_entry`，有单测）：
`MODE_HOST_SIDECAR_ENTRY` 优先且**不做兜底**（显式路径写错就没有入口，由壳发布
`failed(unsupported_runtime)`——静默回落到别的入口会让「配置错」变成「跑的是另一个 Host」）；
没有覆写时按候选顺序取第一个存在的（资源目录 `<resource>/host-sidecar/index.{mjs,ts}` →
debug 构建下的仓库相对路径）。**不硬编码绝对路径**：仓库相对那档只在 debug 构建存在。

## 3. 两个方向的三条命令与失败语义

命令名（前两条与 Task 0 spike 的 `spike_host_send`/`spike_host_recv` **同一对语义**，去掉 `spike_` 前缀；
harness bin 保留原名，两者是不同二进制，互不影响）：

| 命令 | 方向 | 形态 |
| --- | --- | --- |
| `host_send` | 页面 → 壳 → Host | raw body（`InvokeBody::Raw`），整帧写进 Host stdin |
| `host_recv` | Host → 壳 → 页面 | 长轮询，返回 `tauri::ipc::Response`（raw bytes；页面上是 **`ArrayBuffer`**） |
| `host_startup_state` | 壳 → 页面 | 长轮询（`{ since }`），返回 `databaseStartupStateSchema` 形状的状态对象 |

**`host_startup_state` 是新增机制，不是改名**（复评 M12 纠正）：Task 0 的 spike 把
`phase: "ready"` **硬编码在注入桥里**，没有状态通道；DR-T1-3 要求壳能主动推 `failed`，
于是必须有一条「壳 → 页面」的状态流，命令与长轮询的 `since` 语义都是为此新加的。

**三条硬约束**（都有失败现场，见 spike §4）：

1. **端口只能在页面内建**：`tauri 2.12.4`/`wry 0.57.0` 没有 port/transferable 能力。桥在页面里
   `new MessageChannel()`，`port1` 用 `window.postMessage(payload, "*", [port1])` 交给渲染层——
   渲染层拿到的是**真 `MessagePort`**，且 `event.source === window`（`dispatchEvent` 做不到这条）。
2. **入方向必须包 `Uint8Array`**：`MessagePortProtocol` 的收帧判定是 `e.data instanceof Uint8Array`，
   而 Tauri 的 raw 响应是 `ArrayBuffer`；直接投会被**静默丢弃**（RPC 挂到超时，页面看不出原因）。
3. **投递时序是语义**：桥只在 `document.readyState !== "loading"` 之后投递（渲染层的
   `addEventListener("message", …)` 注册在模块脚本里，提前投会白丢一次）。

**失败语义**（DR-T1-3）：sidecar 的失败必须由壳**主动推** `phase: "failed"` + `errorCode`——
状态一旦开始投递，渲染层就再没有自己的超时（§0）。映射（`StartupErrorCode`）：

| 现场 | `errorCode` | 触发点 |
| --- | --- | --- |
| 拉不起进程（node/入口缺失、不可执行） | `open_failed` | `HostHandle::spawn` 返回 `Err` → 壳构造「已失败」句柄 |
| 就绪前退出 / 传输断开（含**就绪后崩**） | `transport_closed` | reader 读 EOF / reaper 收到退出 |
| 就绪期限内没有第一帧 | `startup_status_timeout` | 看门狗（默认 20s；渲染层那个计时器是 30s 且收到状态即拆除） |
| 配置非法（既无入口、也无 node 参数、或数据基目录为空） | `unsupported_runtime` | `HostHandle::spawn` 的配置校验 |

`failedPhase` 一律记 `starting`（壳能判断的最细粒度在 Task 2 之后仍是「拉起 Host」这一段）。
三个监督线程用一个原子量 CAS 决定**谁**能发布终态，所以不会出现「先 failed 后 ready」这种自相矛盾的流。

**Host 内部的启动失败走哪条**（Task 2 的实测口径）：Host 自己在启动序列里抛错时，
它就是「就绪前退出」——结构化落 stderr（`startup_failed phase=failed failedPhase=… errorCode=…`）、
**不写任何帧**、以非零码退出，于是壳的 reaper 报 `failed(transport_closed)`、`failedPhase=starting`。
**渲染层看到的原因比 Host 知道的粗**（Host 报的是 `sql_failed`/`preparing_host_storage`，
渲染层收到 `transport_closed`/`starting`）——这是 §1 登记的差额，不是 bug；「失败被推给渲染层」
这条硬要求已达成（DR-T1-3），「原因精确到阶段」留给 Task 3/4 的 Host → 壳通道。
两半都有自动化：Host 侧 `packages/desktop/tests/startupFailure.test.mts`（错码必须在渲染层
词汇表里、失败时零帧、非零退出），壳侧 `crates/sidecar-host/tests/host_lifecycle.rs`
（就绪前退出 → `failed` + error code）。

### 3.1 「`listen` 抛错会杀死 Host」这条危险的**处置**（复评 C1 → Task 2 落地）

**危险本身没变**：`ChannelServer.onEventListen` 调 `channel.listen(...)` 时**没有 try/catch**
（对照 `onPromise` 会 `try { channel.call } catch { Promise.reject }`）；一次同步抛出会穿出
`onRawMessage` → `Emitter.fire` → stdin 的 `data` 处理函数，**Host 进程当场死亡**，同一端口上的
在途 RPC 全部变 `transport_closed`。Task 1 的 stub 自己踩过，于是把这条写成本任务的必读危险。

**Task 2 真服务面会踩**：真服务由 `ProxyChannel.fromService` 包装，未知事件名**直接抛**。
实测现场（`apps/desktop-tauri/test/realServices.test.mts` 的用例，stderr 留痕）：
`listen_guard setting.onDidChange: … Event not found`。事件名在真服务面里**不由本仓库的一个地方
决定**：`IServiceAccessor` 上有若干**可选**服务（`cuaPermissionService?` /
`windowControllerService?` / `conversationMirrorService?` …），远端与本地提供的面不同，
而渲染层按 host 无关的方式订阅——所以「未知事件名」不是异常输入，是**正常可能的输入**。

**修的位置与理由（不逐个服务打补丁）**：守卫装在 Host 入口的**两个唯一派发边界**上
（`packages/desktop/src/host-standalone/channelGuard.ts`）：

1. `exposeOnChannelServer` 收的那个 `IChannelServer.registerChannel` —— 每个频道在进真
   `ChannelServer` 之前被包一层，`listen` 的同步抛出降级为「永不发事件的 Event + 留痕 + 计数」
   （**含 Host 自己的诊断频道 `host`**——Task 1 复评的 deferred minor「我们自己的 channel 也违反
   自己立的规矩」由此闭合，`realServices.test.mts` 的守卫用例直接订阅 `host.onAnything` 钉住）；
2. 入口喂给 `ChannelServer` 的派发点（`messages.fire(...)`）——协议层/序列化层的任何同步异常
   只被记账，不再带走进程。

为什么不在 `crates/sidecar-host`（壳）里修：壳**只搬字节**，它没有解析 channel 语义的通道，
拿不到「这是哪次 `listen`」；真正未加护的调用点在 `packages/rpc`（Electron 路径共用，本任务不改）。
所以「一次性的、装在派发边界上」的两个位置就是本进程能给的最靠近源头的守卫。

**已知代价（不藏）**：`listen` 被拦下时**调用方看不出订阅失败**（`IServerChannel.listen` 同步
返回 Event，没有错误通道；`IChannelServer` 只暴露 `registerChannel`/`ready`，外部无法替这条请求
回一个 RPC 错误）。所以现在的语义是「安全降级 + 可观察」：不投假事件、stderr 留痕、
计数进 `host.describe().serviceSurface.guardedSubscriptions`。**要把它变成客户端的 RPC 错误，
必须改 `packages/rpc` 的 `onEventListen`**（把 `channel.listen` 的抛出转成 `ResponseType.Error`）
——那超出本任务授权，登记为后续任务的输入。

**Task 2 把它做成了断言，而不是靠人记得**：`apps/desktop-tauri/test/realServices.test.mts` 的
「listen 守卫」用例断言四件事——坏订阅后 Host 仍活着、同一条传输上的**在途 RPC 照常完成**、
一个事件都不投、拒绝可观察（stderr + 计数）；`apps/desktop-tauri/test/portRoundTrip.test.mts`
在**产品入口**上再钉一遍同样的四件事（并发 RPC 版本）。

## 4. 数据隔离与数据根口径（复评 I2）

**`MODE_DATA_BASE_DIR` 是「基目录」，不是「数据根」**：

```
数据根   = join(基目录, ".mode")
基目录   = MODE_DATA_BASE_DIR ?? 家目录（USERPROFILE / HOME）
```

口径来自仓库既有实现，不是本任务发明的：`packages/services/src/paths.ts` 的
`setDataBaseDir` 注释原文是 "Set the base directory for app data (replaces homedir() prefix)"，
`getModeDataRootDir()` 返回 `join(getDataBaseDir(), MODE_DATA_ROOT_DIR_NAME)`（`.mode`）；
`packages/desktop/src/main/desktopRuntimeEnv.ts` 也是把 `dataBaseDir` 原样传给子进程。

- 壳：`data_base_dir_from(MODE_DATA_BASE_DIR, home)`（`crates/sidecar-host/src/lib.rs`，有单测），
  **原样透传**给子进程；`data_root_from_base(base)` 提供 `{base}/.mode` 的派生以供核对。
- Host：`packages/desktop/src/host-standalone/dataRoot.ts` 同一口径，`host.describe` 同时报
  `dataBaseDir`（基目录）与 `dataRoot`（真根）——只报一个正是当初把口径搞反的原因。
- **代价与决策记录**：早期版本把基目录当根用（`dataRoot = MODE_DATA_BASE_DIR ?? home/.mode`），
  后果是真服务会把数据写进 `<base>/.mode/.mode`；Task 1 不写盘所以看不出来，
  Task 2 一接真存储就会落错地方。复评把它钉为产品路径缺陷，本文件记录**采用仓库既有口径**。
- **Task 2 起 Host 真写盘**（Task 1 那条「不写盘」的口径已作废）：真存储会在 `{base}/.mode`
  下建 `v2/tasks-index.sqlite` 与它的 WAL，真服务会写设置/凭据文件。因此隔离断言必须升级形态——
  「隔离目录里不出现目录」现在**必然为假**，把它留着就是一条假断言（Task 1 的 I2 教训：
  「测试把错语义钉死」）。新形态是三条一起断言：
  ① 两条覆写各自生效、根等于 `{base}/.mode`；② **写入确实发生在隔离根里**（否则断言是空的）；
  ③ 用户**真实**的数据根没被碰（比对存在性与条目集合——不比对目录 mtime，别的进程对真实根的
  正常写入会让 mtime 变，那是假红）。
  落点：`apps/desktop-tauri/test/portRoundTrip.test.mts`（产品入口）与
  `apps/desktop-tauri/test/realServices.test.mts`（真服务写的确实是 `{base}/.mode`）。
- **失败是造得出来的、且是确定性的**：数据**基**目录的位置上放一个**文件** → 真存储
  `mkdir {base}` / 打开 sqlite 必然 `ENOTDIR`。实测见 §10.4；这条既当失败语义用例的输入
  （`packages/desktop/tests/startupFailure.test.mts`），也当 opt-in 真机失败路径的输入（§10.6）。
- 测试与本地跑一律 `MODE_DATA_BASE_DIR` + `HOME`/`USERPROFILE` **双覆写**，并**对隔离生效做断言**。

## 5. 覆盖分工（哪些自动化、哪些必须人工）

| 层 | 覆盖方式 | 进 CI |
| --- | --- | --- |
| Host 的进程/监督/帧/启动状态 | `cargo test -p sidecar-host`（15 单测 + 9 集成；**真拉 Node 子进程**） | **是**（Windows 腿，release.yml） |
| 跨语言帧边界（Rust 编码 ↔ Node 解码） | `the_shell_frame_path_reaches_the_host_and_gets_a_reply`：Rust 写一帧真请求、读回真应答 | **是**（同上） |
| 端口往返（真实产品 Host） | `apps/desktop-tauri/test/portRoundTrip.test.mts`：真 Host + 真 `MessagePort` + 真 `ChannelClient`；含「未知命令显式报错」「订阅事件不杀 Host 且带走不了在途 RPC」「坏帧丢弃」「数据隔离（真写入 + 真实根未被碰）」 | **是**（verify job，release.yml） |
| **原生依赖在 sidecar 里可用**（DR-T2-1） | `apps/desktop-tauri/test/nativeDeps.test.mts`：本进程 `node:sqlite` 真读写 + `node-pty` 原生 artifact；**再起生产 Host** 用 `MODE_HOST_NATIVE_DEP_PROBE=spawn` 在 sidecar 进程里**真起一个 pty** | **是**（verify job） |
| **真服务面 / 守卫 / 真启动序列** | `apps/desktop-tauri/test/realServices.test.mts`（5 条）：真命令真返回、`listen` 守卫（Host 存活 + 在途 RPC + 零事件 + 可观察）、stdout 守卫（真服务日志不污染帧流）、数据根口径、启动序列（`ready` 只在频道全注册后发布） | **是**（verify job） |
| **startup 映射（夹具驱动，四条路径）** | `packages/desktop/tests/startupMapping.test.mts`（8 条）+ `packages/desktop/tests/fixtures/startup-storage/*.json`（9 个夹具，覆盖 `starting` / 迁移中 / `ready` / `failed`）；另含**事实路径**（= 生产路径，含帧带不了的 `maintaining`）与**「入口不得自己写应用阶段」的源码扫描**（复评 I1） | **是**（verify job） |
| **Host 启动失败的语义** | `packages/desktop/tests/startupFailure.test.mts`：错码必须落在渲染层 schema 的取值集里、失败时**零帧**、非零退出、不得越过失败点报 ready | **是**（verify job） |
| 纯端口路径回归（Task 0 的） | `test/portMessagePath.test.mts`、`test/hostChannelStdio.test.mts` | **是**（同上） |
| 跨语言字面量（频道名/命令名/状态词汇） | `packages/desktop/tests/host-standalone.test.mts` | **是**（verify job） |
| **打包形态产出得出来、且跑得起来** | `apps/desktop-tauri/scripts/build-host-sidecar.ts` 的自验档：esbuild 打包后用 `node` 直接起产物，走一次 `host.describe` RPC | **否**（构建脚本，人工/发版流程跑；体积数字见 §13） |
| **真 webview 的注入/转移/ACL/时序 + 成功/失败两条路径** | `apps/desktop-tauri/test/webviewRoundTrip.test.mts`（**opt-in，默认 skip**，2 条） | **不能** |
| `mode_tauri::host::{resolve_config, resolve_entry}`（壳侧的候选枚举、家目录、startupId 生成） | **无自动化覆盖**：要测它们就得把 `AppHandle`/具体 runtime 带进测试二进制（§6），只在 opt-in webview 跑里被走到 | 否（**登记在案的缺口**） |
| `host_send`/`host_recv`/`host_startup_state` 的 Tauri 命令**包装层** | 命令体本身没有自动化（本仓不引入 `tauri::test` webview mock 运行时，M1 已定）；其**逻辑**（`send_frame`/`next_frame`/`wait_for_startup_change`/`to_state_json`）由 crate 用例覆盖 | 否（**登记在案的缺口**） |
| 测试 harness 页面（`test/harness/**`）的**类型检查** | 不在任何 tsc 工程里（vite alias 解析到浏览器 shim 与真实渲染层源码，用 tsc 解析会把 node 侧 barrel 拉进 DOM 工程）；构建与运行由 vite/tsx 负责 | 否（**登记在案的缺口**） |

**崩溃重启策略：Task 2 仍然刻意不做，理由与代价写在 §14（DR-T2-4 的处置）**。

### 发版前人工跑一次（发布流程上的硬要求）

```bash
MODE_TAURI_WEBVIEW_ROUNDTRIP=1 node --import tsx --test apps/desktop-tauri/test/webviewRoundTrip.test.mts
```

判据（两条用例都是自动断言，原始输出样例见 §10.5/§10.6）：

- **成功路径**：`status: "ok"`、`gateOpened: true`、`portIsRealMessagePort: true`、
  `initializeObserved: true`、`schemaAccepted: true`、`statePhase: "ready"`、`startupIdMatches: true`、
  `describe.dataBaseDir` 等于临时基目录、`describe.dataRoot` 等于 `{base}/.mode`、
  隔离目录里**出现**真数据根（证明服务面是真的）、进程表里不残留 Host；
- **失败路径**：`status: "startup_failed"`、`statePhase: "failed"`、`failedPhase: "starting"`、
  `errorCode: "transport_closed"`（且整条状态过真 schema）、`gateOpened: false`、
  `startupIdMatches: true`（失败状态与注入桥同代，否则渲染层会把它当陈旧状态丢掉）。

**漏跑这条 = 端口路径或失败推送悄悄坏掉而发布流程说全绿**——这正是本仓库反复吃亏的那类失败
（DR-T1-2 的代价）。

### 人工可观察的开发形态

`node scripts/rust-cargo.ts run -p mode-tauri`（debug）会起真壳：窗口里是 M1 的**命令面验收台**，
底部多了「Host 通道（M3 Task 1）」一屏，显示注入桥的 startupId、端口是否送达、是否真 `MessagePort`、
状态流的 phase/sequence/errorCode，以及端口上旁听到的帧。这一屏是**开发期诊断面**，
Task 3 把 `frontendDist` 换成真 renderer 后仍保留（验收台是命令面回归工具，不删）。
本机命令建议带上隔离覆写，见 §10 的运行记录。

## 6. 为什么 `crates/sidecar-host` 里没有 Tauri 类型（复评 I5 修正过的理由）

**做法**：进程/传输/桥脚本/路径纯函数放在 `crates/sidecar-host`（**零 Tauri 依赖**），
窗口装配与 `#[tauri::command]` 留在壳 crate（`apps/desktop-tauri/src-tauri`，只有 bin 目标）。

**实际观测到的理由**（只写实测支持的，不写推论）：

1. 本 crate 的第一版把窗口装配与命令面放在里面，`cargo test -p sidecar-host` 的**测试二进制**
   以 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` 起不来；`objdump -p` 显示它的导入表里
   同时有 `comctl32.dll` 与 `WebView2Loader.dll`（对照 `crates/platform` 的测试二进制：两者都没有，
   跑得起来）。
2. `crates/tauri-test-manifest`（comctl32 v6 测试清单注入）**当时已经接上**（`build.rs` 调
   `emit_for_tests`，`target/debug/build/sidecar-host-*/out/*.coff.o` 有 `.rsrc$01/.rsrc$02` 段，
   build script 也发出了 `rustc-link-arg-tests`）——但**链出来的测试 exe 里没有 `.rsrc` 段**，
   即那份清单在这个目标上没有生效。**机制未查明**（本次没有再投入；可能是本机 gnu 路线下
   `rustc-link-arg-tests` 与 windres 产物的交互）。
3. `WebView2Loader.dll` 那一半同样未查明：把 `target/debug/WebView2Loader.dll` 拷到
   `target/debug/deps/` 旁边也没解决，所以当时失败的具体入口点无法确定（本机拿不到加载器诊断）。
4. 去掉具体 runtime 引用（`AppHandle` 走泛型、窗口装配移出）后，测试二进制不再导入那两个 DLL，
   用例能真跑——这是**本机工具链下的实测现象**，不是「Tauri 类型一律不能用」：
   `crates/platform` 同样引用 Tauri，但它的测试二进制不导入那两个 DLL。

**代价（接受并登记）**：`mode_tauri::host::resolve_config` / `resolve_entry`（候选枚举、家目录、
startupId 生成）落在 app crate 里，**没有自动化覆盖**（§5 的表里已登记）；能抽成纯函数的部分
（`pick_entry`、`data_base_dir_from`/`data_root_from_base`）已经抽进 crate 并有用例。

## 7. 与既有语义的关系（不许绕过的东西）

- 每窗口一个 window-scoped Local Host：本任务是**一个** Host（单窗口），窗口模型与附件调度是 Task 3。
- 手机远控 / 远程 workspace：本任务不涉及；stub 服务面意味着远端语义也还没接。
- **M1 的命令注册表现在有第二个消费者**（复评 M13）：`platform::commands::init_with_extra`
  是唯一的追加入口（`Builder::invoke_handler` 是覆盖语义），后续任务再加命令时都要走它，
  且 `M1_COMMANDS` 仍是切片表的唯一真源（同名命令会被切片表吃掉）。
- 渲染层零改动：契约的全部价值就是**不需要改渲染层**。若将来发现必须改，那是范围变更。

## 8. 已知边界（写给下一个任务）

1. **桥的 `host_recv` 用有界队列 + 阻塞背压**（默认 256 帧，满了读线程阻塞，不丢帧）；
   `MessagePortFlowControl` 那一层没有用（Task 0 spike §5 的未验证项 3）。
2. **Tauri `Channel<Response>` 的 raw 推送路径未实测**（spike §5 的未验证项 5）——本任务没用到它。
3. **打包形态：本任务已定并实测体积（§13），但「放进安装包」没做**：资源目录里的 sidecar 落点、
   随包的 Node 运行时、`node_modules` 子集的实际排布、以及 **Node 运行时本身的体积**都不在本任务
   范围内（Task 3/5）。§13 的数字**不含** Node 运行时。
4. **macOS / Linux 不在本阶段范围**（Ruling 20）：`taskkill /T` 是 Windows 的收树手段，
   别的平台上靠 Host 的 stdin EOF 保护兜底，未在本阶段验证。打包形态的 `node-pty` 预编译件
   也按 Windows 口径讨论（§13 的裁剪建议）。
5. **没有重启策略**（§14 是 DR-T2-4 的正式处置，含「为什么现在不做」的实测理由）。
6. **分段启动阶段没有传到渲染层**（§1 末段 + §10.6 的原始输出）：Host 报
   `sql_failed`/`preparing_host_storage`，渲染层收 `transport_closed`/`starting`。
7. **`listen` 守卫的拒绝对客户端不可见**（§3.1 的代价）：要变成 RPC 错误得改 `packages/rpc`。
8. **`startupMapping` 的夹具是按声明 schema 构造的**，不是 M2 的真实录制（本 checkout 里
   `packages/shared/test/fixtures/agent-protocol/`、`packages/desktop/tests/agent-runtime-oracle/fixtures/`
   都没有 `startup/storageState` 帧；`grep` 零命中）。它证明「翻译对**声明的协议形状**成立」，
   不证明「对某台机器某次真实运行录下来的字节成立」（§11.3）。
9. **CUA 资产在打包形态里的落点未验**：探针只覆盖「注入了但不存在 = 失败」（§12.1）。
10. **`host.describe().startup.history` 是本进程的诊断事实**，不是渲染层可见的状态（同上第 6 条）。
11. **磁盘摘要没实现**：状态里的 `disk` 恒为 `[]`（schema 要求该字段存在）。将来运行时给出
    `StartupDiskSummary` 时，谁填它、谁负责合并没有定（复评 M2，登记在 §15.3）。
12. **`sequence` 的拥有者未定**：Host 与壳各自从 0 自增，而渲染层按 `sequence` 丢弃旧状态；
    Task 3/4 合并两股流之前必须先定唯一拥有者（复评 M6，登记在 §15.3）。

## 9. 纪律登记（clean-room）

- `crates/sidecar-host/**`、`apps/desktop-tauri/**` 全部新写；
- 上游衍生文件里**只读接口面**：`packages/rpc` 的导出签名与类型声明（`MessagePortLike`、
  `ChannelServer` 的构造与 `ready()`、`IServerChannel`、`Event`/`toDisposable`、`VSBuffer`、
  `BufferReader`/`deserialize`）、`packages/shared` 的常量与 zod schema（`InternalChannels`、
  `ServiceChannels`、`databaseStartupStateSchema` / `modeStorageStartupStateSchema` 及其错误码/阶段枚举、
  `DatabaseMigrationFacts`、`classifyDatabaseStartupError`）、
  `packages/services/src/paths.ts` 的**数据根口径**（`getDataBaseDir`/`getModeDataRootDir`）、
  `packages/desktop/src/renderer/src/databaseStartupAdmission.ts`（**作为未改动源码被集成测试与
  验收台 import**，不读实现体做重写）；
- 复评引用到的一处上游实现细节（`ChannelServer.onEventListen` 没有 try/catch）是**复评给出**
  并由我用真子进程复现的（§10 的 C1 现场），不是照抄实现；
- 帧 envelope 的形状（首字节是序列化层标签而非 `RequestType`）是用 `@mode/rpc` 的公开
  反序列化面**跑出来**的（`deserialize` 真客户端帧 → `[100, 0, "host", "describe"]`）；
- 渲染层零改动：`git diff` 对 `packages/desktop/src/renderer/**` 为空。

### 9.1 Task 2 的追加登记（真服务面这一轮）

- **`ProxyChannel.fromService` 的 `listen` 对未知事件名会抛**这条：**没有读实现体**，
  是在 `apps/desktop-tauri/test/realServices.test.mts` 里用**真服务入口 + 真订阅**观测到的
  （stderr 的 `listen_guard … Event not found`）。文件（`packages/rpc/src/proxy-channel.ts`）未打开。
- **真服务面的装配知识来自公共入口的签名**：`@mode/services/node` 的
  `createLocalServices` / `disposeServiceResourcesAndWait` 与 `ServiceCollection.exposeOnChannelServer`
  的**调用形状**（用 `host.describe().serviceSurface.channelCount = 40` 与真命令返回真值**验证**，
  不是照抄装配代码）；逐个 `createXxxService` 的构造参数**没有**读——那正是本任务不 deep import
  的理由（§11.1）。
- **`prepareTasksIndexStorage(path, onPhase)` 的回调形状**（阶段名 `checking`/`waiting_for_lock`/
  `migrating`/`committing`/`ready` + 迁移事实）取自 `@mode/services/storage-startup` 的**导出签名**；
  它在产品入口里的**使用**经真机跑验证（§10.5 的 `storage_prepared` 与 `startup.history`）。
- **数据**：`MODE_DATA_BASE_DIR` 语义来自 `packages/services/src/paths.ts` 的注释与
  `getModeDataRootDir()` 的返回式（Task 1 已登记），Task 2 用**真服务写出的实际路径**
  再验证一次（§10.5 的 `dataRoot`）。
- **未读**：`packages/services/**` 的任何函数体、`packages/desktop/src/host/index.ts`（Electron 那侧的
  装配）、`packages/desktop/src/renderer/src/main.tsx` 的实现体。Task 2 需要知道「渲染层要什么形状」
  的部分全部由 `@mode/shared` 的 zod schema 与 `databaseStartupAdmission` 的**类接口**给出。

## 10. 证据登记（可复核的原始输出）

复评要求「opt-in 跑的原始输出不能只活在报告里」，所以这里逐条登记命令与实测输出
（截取关键行，长度与字段都是原文）。

### 10.1 C1 的失败现场与修复后（`stub.listen` 抛错会杀死 Host）

```
$ cd apps/desktop-tauri && node --import tsx --test test/portRoundTrip.test.mts   # 修复前
✖ 订阅 stub 服务事件不会杀死 Host，且不给「订阅成功」的假象
  AssertionError [ERR_ASSERTION]: 订阅服务事件后 Host 必须还活着；stderr:
  [mode-host-sidecar] ready startupId=port-round-trip-startup-1 … channels=43 node=v24.18.1
  D:\ZCodium\packages\desktop\src\host-standalone\stubAccessor.ts:44
        throw notImplementedError(channelName, `${event}（listen）`);
  Error: not_implemented: file.onDidChange（listen） 尚未实现
      at Object.listen (…/stubAccessor.ts:44:13)
      at ChannelServer.onEventListen (…/channelServer.ts:206:32)
      at ChannelServer.onRawMessage (…/channelServer.ts:85:14)
      at Emitter.fire (…/foundation.ts:159:7)
      at <anonymous> (…/host-standalone/index.ts:54:16)
      at Socket.<anonymous> (…/stdioFrames.ts:91:7)
  1 !== null
```

修复后（同一命令）：

```
✔ 生产 Host sidecar：Initialize 与一次真 RPC 往返可以走端口 (656.2929ms)
✔ stub 服务面对每个命令都显式 not_implemented（不返回 undefined / 空对象） (674.2103ms)
✔ 订阅 stub 服务事件不会杀死 Host，且不给「订阅成功」的假象 (1162.1653ms)
✔ Host 入口拒绝形状不合法的帧，不会把 ChannelServer 带崩 (966.373ms)
✔ 数据隔离：两条覆写都生效，且运行后不产生真实数据目录 (1516.7822ms)
ℹ tests 5   ℹ pass 5   ℹ fail 0
```

### 10.2 opt-in 真 webview 往返（产品壳 + 产品 Host，fix round 1 后重跑）

```bash
$ MODE_TAURI_WEBVIEW_ROUNDTRIP=1 node --import tsx --test apps/desktop-tauri/test/webviewRoundTrip.test.mts
```

```
HARNESS host startupId=host-12576-1792143819911 pid=6468
ROUNDTRIP_PAGE_LOG page_loaded
ROUNDTRIP_PAGE_LOG port event.source===window=true ports.length=1 real_message_port=true databaseStartupId=host-12576-1792143819911
ROUNDTRIP_PAGE_LOG state phase=starting startupId=host-12576-1792143819911
ROUNDTRIP_PAGE_LOG state phase=ready startupId=host-12576-1792143819911
ROUNDTRIP_PAGE_LOG gate_opened port_is_message_port=true
ROUNDTRIP_PAGE_LOG rpc_ok name=mode-host-sidecar pid=6468
ROUNDTRIP_PAGE_REPORT {"status":"ok","gateOpened":true,"portIsRealMessagePort":true,
  "initializeObserved":true,"schemaAccepted":true,"statePhase":"ready",
  "stateStartupId":"host-12576-1792143819911","startupIdMatches":true,
  "describe":{"name":"mode-host-sidecar","pid":6468,"nodeVersion":"v24.18.1","platform":"win32",
    "startupId":"host-12576-1792143819911",
    "dataBaseDir":"C:\\Users\\…\\mode-host-webview-zMgCUB\\data",
    "dataRoot":"C:\\Users\\…\\mode-host-webview-zMgCUB\\data\\.mode",
    "dataBaseDirSource":"MODE_DATA_BASE_DIR","rejectedFrames":0,"stubEventSubscriptions":0,
    "serviceChannels":[42 个名字，file…conversation-mirror]}}
HARNESS watchdog_exit
HARNESS host_shutdown pid=6468
✔ 真 webview：产品壳 + 产品 Host 让渲染层的启动闸门放行，且退出不留孤儿 (29602.1618ms)
ℹ tests 1   ℹ pass 1   ℹ fail 0
```

「闸门」是**渲染层未改动**的 `DatabaseStartupAdmission`（vite alias 到真源码），状态过真
`databaseStartupStateSchema`；`dataRoot` 那一行是 fix round 1（I2）之后的形态：`{base}/.mode`。

### 10.3 产品壳真跑一次（验收台面板 + 强杀后的残留检查）

```bash
$ MODE_DATA_BASE_DIR=D:/tmp/mode-local-run2/data HOME=D:/tmp/mode-local-run2/home \
  USERPROFILE=D:/tmp/mode-local-run2/home ./target/debug/ModeTauri.exe
[mode-tauri] host startupId=host-9744-1792144250986 pid=7448
[mode-host-sidecar] ready startupId=host-9744-1792144250986 dataBaseDir=D:/tmp/mode-local-run2/data \
  dataRoot=D:\tmp\mode-local-run2\data\.mode source=MODE_DATA_BASE_DIR channels=43 node=v24.18.1
```

窗口文本（UI Automation 读 Document 文本，脚本 `.plan/uia-doc.ps1`）：

```
Host 通道（M3 Task 1）
端口 + database-startup 状态流：渲染层启动闸门的两条输入
[09:50:51.402] 注入桥：startupId=host-9744-1792144250986 端口已投=false 收帧=0 状态代=(未收到) phase=(未收到)
[09:50:51.464] service-port：databaseStartupId=host-9744-1792144250986 event.source===window=true ports.length=1 真 MessagePort=true
[09:50:51.473] database-startup-state：phase=starting startupId=host-9744-1792144250986 sequence=0
[09:50:51.692] 端口帧：[object Uint8Array] bytes=6
[09:50:51.694] database-startup-state：phase=ready startupId=host-9744-1792144250986 sequence=1
```

强杀壳（`Stop-Process -Force`，**绕过** `RunEvent::Exit`）后：

```
> @(Get-CimInstance Win32_Process -Filter "Name='node.exe'" |
    Where-Object { $_.CommandLine -like '*host-standalone*' }).Count
0
> @(Get-Process ModeTauri -ErrorAction SilentlyContinue).Count
0
```

即「被强杀也不留孤儿」由 Host 自己的 stdin EOF 保护兜住（第二道保险）；隔离目录
`D:/tmp/mode-local-run2/` 下只有一个空的 `home`——**基目录与 `.mode` 都没被创建**（Task 1 不写盘）。
> 注：上面 §10.1–§10.3 是 **Task 1**（stub 时代）的证据，保留是因为它们是 C1/I2 的失败现场；
> 其中「stub」相关的形态在 Task 2 已被真服务面取代（§10.7 起是 Task 2 的证据）。
> §10.3 那句「基目录与 `.mode` 都没被创建」只在 Task 1 成立，Task 2 起 Host 会写盘（§4）。

---

## Task 2 的证据（§10.4–§10.7）

### 10.4 原生依赖在 sidecar 里可用（DR-T2-1 的判定）

```bash
$ cd apps/desktop-tauri && node --import tsx --test test/nativeDeps.test.mts
```

```text
✔ 原生依赖在 sidecar 的 loader 下可加载：node:sqlite 真读写、node-pty 模块与原生 artifact 到位 (11.9945ms)
✔ CUA 资产路径：未注入不是失败，注入了不存在的路径必须是失败 (1.5176ms)
✔ 生产 Host sidecar 自己报告原生依赖可用，并真起一个 pty（同一进程内的证据） (3306.8321ms)
ℹ tests 3   ℹ pass 3   ℹ fail 0
```

sidecar 进程内自报的那份（`host.describe().nativeDeps`，真机跑的原文见 §10.5）：

```json
"nativeDeps":{"sqlite":{"ok":true,"version":"3.53.1","roundTrip":41},
 "pty":{"ok":true,"spawnVerified":false,"nativeArtifacts":["prebuilds\\win32-x64\\conpty.node","prebuilds\\win32-x64\\pty.node"]},
 "cua":{"injected":false,"ok":false}}
```

`roundTrip: 41` 是写进去再读回来的值（不是「import 没抛错」）；`spawnVerified: false` 是**默认档**的
如实形态（产品启动不真起 pty），深探测档（`MODE_HOST_NATIVE_DEP_PROBE=spawn`）在用例里真起一个 pty
并断言 `exitCode=0` + 输出里有 `pty-ok`。**结论：DR-M3-1 的前提成立，不需要重开决策。**

### 10.5 opt-in 真 webview（成功路径）——真启动驱动闸门

```bash
$ MODE_TAURI_WEBVIEW_ROUNDTRIP=1 node --import tsx --test apps/desktop-tauri/test/webviewRoundTrip.test.mts
```

```text
HARNESS host startupId=host-4772-1792147712344 pid=12944
ROUNDTRIP_PAGE_LOG port event.source===window=true ports.length=1 real_message_port=true databaseStartupId=host-4772-1792147712344
ROUNDTRIP_PAGE_LOG state phase=starting startupId=host-4772-1792147712344
[mode-host-sidecar] native_deps sqlite=ok(3.53.1) pty=ok(spawnVerified=false) cua=not-injected
[mode-host-sidecar] storage_prepared path=C:\…\mode-host-webview-6hfz4r\data\.mode\v2\tasks-index.sqlite
[2026-10-16 18:48:34.391] [pid:12944] [cua-pip-session] [cua-pip-session] wiring resolved { enabled: false, … }
[mode-host-sidecar] services_ready channels=40 builtinConfig=env(exists=true)
[mode-host-sidecar] ready startupId=host-4772-1792147712344 dataBaseDir=C:\…\data dataRoot=C:\…\data\.mode source=MODE_DATA_BASE_DIR channels=40 node=v24.18.1 shutdown=false
ROUNDTRIP_PAGE_LOG state phase=ready startupId=host-4772-1792147712344
ROUNDTRIP_PAGE_LOG gate_opened port_is_message_port=true
ROUNDTRIP_PAGE_LOG rpc_ok name=mode-host-sidecar pid=12944
ROUNDTRIP_PAGE_REPORT {"status":"ok","gateOpened":true,"portIsRealMessagePort":true,"initializeObserved":true,
 "schemaAccepted":true,"statePhase":"ready","stateStartupId":"host-4772-1792147712344","startupIdMatches":true,
 "describe":{"name":"mode-host-sidecar","pid":12944,…,
  "serviceChannels":["file","media-preview","git","git-checkpoint","system","terminal","setting",…,"model-selection"],
  "serviceSurface":{"kind":"services","channelCount":40,…,"stdoutDiverted":1,
   "stdoutSamples":["[2026-10-16 18:48:34.391] [pid:12944] [cua-pip-session] …"]},
  "nativeDeps":{…},
  "startup":{"phase":"ready","sequence":11,"attemptId":"attempt-host-4772-1792147712344",
   "history":["1:preparing_host_storage","2:preparing_host_storage",…,"9:preparing_host_storage",
              "10:starting_services","11:ready"]}}}
HARNESS watchdog_exit
HARNESS host_shutdown pid=12944
✔ 真 webview（成功路径）：产品壳 + 产品 Host 让渲染层的启动闸门放行，且退出不留孤儿 (30212.0389ms)
```

三点值得单独指出：① `startup.history` 是**真阶段序列**（存储 → 服务 → ready），不是合成值；
② `stdoutDiverted: 1` + 样本是 `cua-pip-session` 的日志 —— 那条守卫在真机上确实拦到了
一条**会冲散帧流**的写入（没有它，这次往返会以「RPC 全挂」告终）；③ `serviceChannels` 是 40 个
**真服务**频道，`rpc_ok` 说明渲染层的接收形状下真业务命令能走通。

### 10.6 opt-in 真 webview（失败路径）——`failed` 是到达渲染层的唯一失败信号

同一个文件里的第二条用例，把数据**基**目录的位置放成一个**文件**（`ENOTDIR` 是确定性的）：
**产品 Host 入口、产品壳、产品桥，没有替身**。

```text
HARNESS host startupId=host-9928-1792147739290 pid=12860
ROUNDTRIP_PAGE_LOG state phase=starting startupId=host-9928-1792147739290
[mode-host-sidecar] startup_failed phase=failed failedPhase=preparing_host_storage errorCode=sql_failed error=ENOTDIR: not a directory, mkdir 'C:\…\mode-host-webview-Ky72BX\data'
[sidecar-host] host 停止：host_stdout_eof
ROUNDTRIP_PAGE_LOG state phase=failed startupId=host-9928-1792147739290
ROUNDTRIP_PAGE_REPORT {"status":"startup_failed","gateOpened":false,"portSeen":true,"portIsRealMessagePort":true,
 "initializeObserved":false,"schemaAccepted":true,"statePhase":"failed",
 "stateStartupId":"host-9928-1792147739290","failedPhase":"starting",
 "errorCode":"transport_closed","startupIdMatches":true}
✔ 真 webview（失败路径）：真启动失败时页面收到 failed + errorCode，闸门不放行 (27000.6566ms)
```

**这张输出同时证了两件事和一条差额**：① 失败**到达**了页面（`phase=failed` + schema 通过 +
同代匹配），而按 §0 的口径，这是渲染层此后唯一的失败信号；② 闸门**没有**放行
（`gateOpened:false`、`initializeObserved:false`）；③ **差额**：Host 知道的是
`sql_failed`/`preparing_host_storage`，渲染层收到的是 `transport_closed`/`starting`
——壳只能判断「就绪前退出」。这条差额已登记（§1、§8.6），修它要一条 Host → 壳的通道（Task 3/4）。

### 10.7 CI 里的那几条（真服务面 / 守卫 / 启动失败 / 映射）

```bash
$ node --import tsx --test packages/desktop/tests/startupMapping.test.mts
✔ starting：状态机初值就是 starting，且字段全过 schema (7.088ms)
✔ 迁移中：库内阶段进 databasePhase，应用阶段由 databaseKind 决定（两个库各一条） (3.9442ms)
✔ ready：单个库 ready 不等于应用 ready（应用 ready 只在服务面装配完成后发布） (0.7119ms)
✔ failed：errorCode + failedPhase + 细节码都必须能落到渲染层读的字段上 (0.584ms)
✔ failed（本机路径）：异常用仓库既有分类，不自己编错码；无分类依据时落 sql_failed (0.5873ms)
✔ 全部夹具都覆盖到位：每个 databaseKind × 关键库内阶段都能翻译，且没有未映射的 kind (1.6507ms)
ℹ tests 6   ℹ pass 6   ℹ fail 0

$ node --import tsx --test packages/desktop/tests/startupFailure.test.mts
✔ 真启动失败：Host 报 failed + errorCode、不写任何帧、以非零码退出 (992.1253ms)
ℹ tests 1   ℹ pass 1   ℹ fail 0

$ cd apps/desktop-tauri && node --import tsx --test test/realServices.test.mts test/portRoundTrip.test.mts
✔ 真实服务面：真实业务命令经端口返回真值（不再是 stub 的 not_implemented） (2218.5192ms)
✔ 真实服务的 listen 守卫：订阅一个不存在的事件不会杀死 Host，也不给「订阅成功」的假象 (2772.1406ms)
✔ stdout 守卫：真实服务写 stdout 不会污染帧流，字节被转到 stderr 并计数 (2199.9589ms)
✔ 数据根口径：真实服务的写入落在 {base}/.mode，不出现 {base}/.mode/.mode (2480.8274ms)
✔ 真启动序列：ready 由真存储 + 真服务装配推进，且只在全部服务频道注册之后发布 (…)
✔ 生产 Host sidecar：Initialize 与一次真 RPC 往返可以走端口 (2100.2688ms)
✔ 真服务面对未知命令显式报错（不返回 undefined / 空对象），stub 的 not_implemented 已消失 (2199.7279ms)
✔ 订阅未知事件不杀 Host，也不带走同一条传输上的在途 RPC (2382.8259ms)
✔ Host 入口拒绝形状不合法的帧，不会把 ChannelServer 带崩 (2436.2006ms)
✔ 数据隔离是真的：两条覆写各自生效，真写入落在隔离根，用户真实 ~/.mode 未被触碰 (4251.2267ms)
```

---

## 11. 真服务面与启动序列（Task 2 · DR-T2-2 的落点）

### 11.1 装配口径：走公共入口，不 deep import

`packages/desktop/src/host-standalone/serviceAccessor.ts` 用 `@mode/services/node` 的
`createLocalServices({ modeBuiltinProviderConfigFilePath })` 一次装配全部服务，再用
`services.exposeOnChannelServer(guardedServer)` 把频道交出来（`guardedServer` 是套了 §3.1 守卫的
`IChannelServer`）。**不逐个 `createXxxService`**：45 个服务各有构造参数与相互依赖，手写装配等于把
装配知识抄一份到本包——那份抄写就是漂移源，而且必然牵到 `#src/...` 内部路径（违反「不 deep import」）。

**与 Electron Host 的装配差异（登记）**：Electron 侧在 app 层还传了窗口级 `overrides`，
以及 `parentPort`、`settingService`、`hostApiNetworkTransport` 等注入点。本任务只做**服务面本身**；
缺省注入下这些能力各自表现为「不可用/空操作」（`parentPort` 传 null 时广播为空操作），
不是静默走另一条路径。窗口模型与附件调度是 Task 3。

### 11.2 真启动序列（Host 进程内）

```text
starting                  ← 状态机初值（还什么都没做）
  preparing_host_storage  ← prepareTasksIndexStorage：真开 DatabaseSync、真跑迁移，
                             按库内阶段回调 checking / waiting_for_lock / migrating / maintaining /
                             committing / ready + 迁移事实（真机实测 9 次上报，见 §10.5）
  starting_services       ← createLocalServices + exposeOnChannelServer（40 个频道）
ready                     ← 全部频道注册进 ChannelServer 之后（deferInit 的 server.ready()）
```

任一步抛出 → `failed` + `errorCode`（`classifyDatabaseStartupError`，不自己编分类）、
结构化写 stderr、`process.exit(1)`（§3 的口径与 §10.6 的现场）。

### 11.3 `startup/storageState` → `DatabaseStartupState` 的**显式翻译**（DR-T2-2）

两个词汇表看起来像，其实不是一回事：

| 来源 | phase 取值 | 说的是 |
| --- | --- | --- |
| 运行时 `startup/storageState` 帧 | `checking` / `waiting_for_lock` / `migrating` / `committing` / `ready` / `failed` | **一个数据库**此刻在干什么 |
| 渲染层 `DatabaseStartupState` | `starting` / `preparing_host_storage` / `preparing_session_storage` / `starting_services` / `ready` / `failed` | **整个应用启动**走到哪一步 |

映射规则（`packages/desktop/src/host-standalone/startupMapping.ts` 的 `StartupStateMachine`）：

1. **应用阶段由 `databaseKind` 决定**（`tasks-index` → `preparing_host_storage`，
   `session` → `preparing_session_storage`；i18n 文案分别是「准备任务索引 / 准备聊天记录」）；
   枚举未来加值时**必须显式决定**（`never` 检查，不静默落进某个分支）；
2. **库内阶段原样进 `databasePhase`**（两边声明的公共部分逐字一致）；
3. **运行时的 `ready` 只把 `databasePhase` 推到 `ready`，不把应用阶段推成 `ready`** ——
   应用 `ready` 由 Host 自己（服务面装配完成）说了算：一个库 ready 不等于应用 ready；
4. **失败**：`failed` 带 `errorCode`（帧 schema 的 `superRefine` 强制）→ 应用 `failed` +
   `failedPhase`（失败停在哪个应用阶段）+ 同码 + 细节码（`sqliteCode`/`systemCode`/`migrationId`）；
5. **`migration`（`DatabaseMigrationFacts`）两边是同一个 schema**，原样带过；正在迁移/提交时
   另填 `currentMigration` 与 `migrationBaselines`（渲染层的重试判据读它）；
6. 状态机的**每个输出都过一次 `databaseStartupStateSchema.parse`**：在某个字段写错的**那一刻**就红，
   而不是等渲染层因 `.strict()` 把整条消息静默丢掉（症状是「闸门不放行、也没有失败原因」）。

**`sequence` 不直通**：状态自身的 `sequence` 是**本机发布计数器**（从 0 单调递增，与壳的
`StartupMonitor` 同义，渲染层按它丢弃旧状态）；帧里那个 `sequence` 是**单库流**的序号。
直通会在两个库交替上报时倒退，而「旧状态不得覆盖新状态」是消费侧的语义。

**夹具**：`packages/desktop/tests/fixtures/startup-storage/*.json`（9 个），覆盖四条路径——
`starting`（状态机初值）、迁移中（`checking`/`waiting_for_lock`/`migrating`/`committing` × 两个
`databaseKind`）、`ready`（两个库各一条 + 应用 ready 由 Host 发布）、`failed`（权限、锁超时，
含 `sqliteCode`/`systemCode`/`migrationId`）。每个夹具**先过 `modeStorageStartupStateSchema`**
才有资格当输入，每个输出**再过 `databaseStartupStateSchema`**（§8.8 的差额：夹具是按声明 schema
构造的，不是 M2 的真实录制——本 checkout 里没有那份录制）。

---

## 12. 原生依赖与 Built-in Provider Config（Task 2 · DR-T2-1）

### 12.1 三类原生资产怎么证

`packages/desktop/src/host-standalone/nativeDeps.ts`（**生产代码里的探针**，不只是测试）：

| 资产 | 默认档（产品启动跑） | 深探测档（`MODE_HOST_NATIVE_DEP_PROBE=spawn`，测试用） |
| --- | --- | --- |
| `node:sqlite` | **真读写**：`:memory:` 建表 → 插入 41 → 查回 → 读 `sqlite_version()`（「import 没抛错」证明不了语句执行链路可用） | 同左 |
| `node-pty` | 模块加载 + API 面（`spawn`/`fork`/`createTerminal`）+ **原生 artifact 存在性**（`build/Release` → `build/Debug` → `prebuilds/<platform>-<arch>`，与 node-pty 自己的搜索顺序一致） | 再**真起一个 pty** 跑 `cmd /c echo pty-ok`，断言退出码与输出 |
| CUA 资产（经 `MODE_CUA_BUNDLED_HELPER_APP_PATH` 注入） | 未注入 = **不是失败**（opt-in 特性缺省关闭）；注入了不存在的路径 = **失败**（否则「配置写错」会被读成「CUA 正常但没启用」） | 同左 |

**为什么探针在生产代码里**：DR-M3-1 选 Node sidecar 的唯一理由是复用存量 `packages/services`，
那层依赖 ABI 敏感的原生资产；「原生依赖到底好不好」若是只有测试知道的事实，产品每次启动时
它就是不可观察的——而这是「判错就要重开决策」的量级。探针**非致命**（失败只记账并上报）：
一个原生依赖坏掉仍能渲染大部分 UI，比「Host 直接不启动」对用户更有利；真正用到它的服务自己会抛，
那才是它该失败的地方。结果在 `host.describe().nativeDeps`，用例断言它与真机行为一致（§10.4）。

**Windows 上为什么必须有「真起进程」那一档**：node-pty 在 win32 **没有**任何「加载绑定」的公开入口
（`exports.native` 在 win32 恒为 `null`，真加载只发生在 `spawn` 里），所以「模块与 artifact 在」
证不了绑定可用；而为了自检在每次产品启动时真起一个 shell 是有副作用的动作。两档分别把话说清楚，
不用一个布尔占位掩盖差别。

### 12.2 Built-in Provider Config 的落点

`resolveBuiltinProviderConfig`（`serviceAccessor.ts`）按**显式覆写 → 打包资源目录 → 开发态仓库相对**
解析，并**把命中来源一起报出来**（`source: "env" | "resources" | "repo"`）——三个候选都可能给出一个
「看起来对」的路径，只有来源能定位配置错误。**不 import** `packages/desktop/src/main/desktopProviderConfig.ts`：
那份实现 import 了 `electron`，在 sidecar 里不可用（「无 Electron」正是本包存在的理由之一）。

---

## 13. 打包形态与体积（Task 2 · DR-T2-3）

**决定**：sidecar 打包成**单文件 ESM bundle**（esbuild），**原生/运行时依赖保持 external**
并随包带它们的 `node_modules` 子集。落点照壳侧候选枚举：`<resource>/host-sidecar/index.mjs`。
external 清单一字不改地取自 Electron 那条已经跑在生产上的清单
（`packages/desktop/tsup.config.ts` 的 `desktopNodeRuntimeExternals`）——同一批服务在另一条宿主
路径上验证过的取舍，不是本任务新发明的。

两个候选的取舍见 `apps/desktop-tauri/scripts/build-host-sidecar.ts` 的文件头；要点是**可判定**：
bundle 后运行时依赖面收敛成一个文件 + 一张显式 external 清单，而「随包带源码树」的依赖面是
「此刻恰好能解析」，出问题的症状是启动期才暴露的解析错误。

**实测（2026-10-16，本机 Windows；fix round 1 重跑，含传递闭包）**：

```bash
$ node --import tsx apps/desktop-tauri/scripts/build-host-sidecar.ts
[host-sidecar] 打包形态 = 单文件 ESM bundle + external 的 node_modules 子集
[host-sidecar] bundle 体积   5106411 B (4.87 MiB)
[host-sidecar] externals 目录（直接 external，own-dir，**下界**） 96389444 B (91.92 MiB)
[host-sidecar]   - node-pty@1.1.0 65621790 B (62.58 MiB)
[host-sidecar]   - @larksuiteoapi/node-sdk@1.61.1 25515934 B (24.33 MiB)
[host-sidecar]   - node-forge@1.4.0 1647637 B (1.57 MiB)
[host-sidecar]   - undici@6.24.1 1184934 B (1.13 MiB)
[host-sidecar]   - ssh2@1.17.0 1119022 B (1.07 MiB)
[host-sidecar]   - yaml@2.9.0 685953 B (0.65 MiB)
[host-sidecar]   - @wecom/aibot-node-sdk@1.0.7 516499 B (0.49 MiB)
[host-sidecar]   - yauzl@3.3.0 97675 B (0.09 MiB)
[host-sidecar] externals 闭包（含传递依赖，去重 66 个包） 107593418 B (102.61 MiB)
[host-sidecar]   + protobufjs@7.5.5 2949726 B (2.81 MiB)
[host-sidecar]   + axios@1.13.6 2423319 B (2.31 MiB)
[host-sidecar]   + @types/node@25.6.0 2349988 B (2.24 MiB)
[host-sidecar]   + cpu-features@0.0.10 576021 B (0.55 MiB)
[host-sidecar]   + nan@2.26.2 453821 B (0.43 MiB)
[host-sidecar]   + node-addon-api@7.1.1 396312 B (0.38 MiB)
[host-sidecar]   + qs@6.15.1 312854 B (0.30 MiB)
[host-sidecar]   + mime-db@1.52.0 205539 B (0.20 MiB)
[host-sidecar]   + …另有 50 个传递依赖
[host-sidecar] ⚠ 解析不到的依赖（数字因此偏小）：long
[host-sidecar] 合计（bundle + externals 闭包） 112699829 B (107.48 MiB)（**不含 Node 运行时本身**）
[host-sidecar] 自验          mode-host-sidecar channels=40 builtinConfig=resources(exists=true)
```

**两列数字的区别（复评 I2）**：`own-dir` 那一列只加**直接 external** 的包目录，
在本 checkout 的 hoisted 布局下**不含**被提升上去的传递依赖，所以它只是**下界**；
「闭包」那一列按 `package.json` 的 `dependencies`/`optionalDependencies` 逐层解析、
以解析到的 package.json 路径去重后求和（跳过多层嵌套的 `node_modules` 以免双计），
66 个包 / **102.61 MiB** 才是打包形态实际要带的量。一个解析不到的依赖（`long`，`protobufjs`
的可选依赖，本 checkout 未安装）单独列出来——**不静默**，否则数字会偏小而看起来正常。
（复评给的闭包是 102.98 MiB，与此处 102.61 MiB 的差额=测量法不同：本脚本跳过嵌套
`node_modules` 并把未安装的 `long` 列为 unresolved。）

**门禁口径（复评 C1 · Ruling 26：已拆成两份预算）**：产品阶段原来的单一「核心包 ≤30MB」门禁
在选定架构下**不可达**（单是 `node.exe` gzip 后 33.91 MiB 就越过 30MB），因此拆成
**(a) 核心**（壳 + UI 资产 + Host bundle，沿用 ≤30MB；Host bundle 本任务实测 4.87 MiB）与
**(b) Node 运行时 + sidecar externals**（基准：闭包 102.61 MiB + `node.exe` 88.25 MiB / gzip 33.91 MiB，
门禁数值由 Task 5 定）。完整的算术、代价与「唯一能兑现原目标的替代方案（Rust 重写 Host）」
登记在阶段计划 `docs/plans/2026-10-16-tauri-m3-ecosystem.md` 的「体积门禁拆分登记」——
**本文件的数字是那两份预算的当前基准，不是失败**。

自验是**默认开**的：脚本用 `node <bundle>`（不带 tsx）起产物，走一次 `host.describe` 的 RPC 往返，
断言 `serviceSurface.kind === "services"`，**并断言 Built-in Provider Config 走的是「资源目录」档**
（`builtinProviderConfigSource === "resources"` 且文件存在——复评 M3：自验刻意清掉
`MODE_BUILTIN_PROVIDER_CONFIG_FILE`、改设 `MODE_HOST_RESOURCES_DIR` 指向一份临时资源目录，
否则本机装了 Mode 时会在 env 档的绿灯下报「打包形态 OK」，而打包形态真正要用的那一档从没被走过）。
否则「打包形态」只是一次成功的文件写入，而本仓库反复吃亏的正是「产物生成了，但没人跑过它」。

**交给 Task 5 的三条实测线索**（数字是量出来的，不是估的；**MB/MiB 一律按 1024 进制**）：

1. `node-pty` 的 62.58 MiB 里 **57.82 MiB 是 `prebuilds/`**（= 60,632,144 B），其中
   `win32-x64` **29.65 MiB**、`win32-arm64` **27.98 MiB**、`darwin-*` **0.19 MiB**。
   Windows-only（Ruling 20）下裁掉非本平台预编译件是纯收益（原始省 0.19 MiB）；
   连 `win32-arm64` 一起裁（本阶段没把 Windows ARM64 当目标）**原始**省 28.17 MiB，
   则 node-pty ≈ **34.41 MiB**、externals 闭包 ≈ **63.75 MiB**。
   **但安装包里的收益要按压缩比打折**：`.node` 约 **4.3:1**，所以那 28.17 MiB 在安装包里
   只值 **~6.6 MiB**。**裁不裁、裁到什么平台由 Task 5 与安装包平台矩阵一起定**（本任务不替它决定）。
2. 闭包里有两块**运行期不需要**的东西，值得 Task 5 单独评估：`@types/node` **2.24 MiB**
   （某个包把类型包写进了 `dependencies`）与 `protobufjs` 的未安装可选依赖 `long`。
3. `@larksuiteoapi/node-sdk` 24.33 MiB 必须走 external（链路上有动态 `require("util")`）。
   要更小得评估「只带用到的子路径」或换打包方式——**不要在门禁上妥协**。

**踩到的坑（值得下一个任务知道）**：内联进来的 CJS 依赖里有**动态** `require("events")`，
esbuild 的 ESM 产物把它留成 `__require`，运行时报
`Dynamic require of "events" is not supported`——而且是在**服务面装配那一步**才炸
（`startup_failed … failedPhase=starting_services errorCode=sql_failed`，存储已经开好了）。
banner 里给产物一个真的 `require`（`createRequire(import.meta.url)`）即可收口，
`__require` 的判定正是 `typeof require !== "undefined" ? require : 抛错`。**这条只有真跑产物才看得见**
——又一次印证本仓库那条纪律：产物必须被跑过。

**单位口径的订正（复评 M1）**：本节上一版把 `prebuilds` 子树按 **10^6** 算却标成 MiB
（写成 60.63「MiB」而真值是 57.82 MiB），连带把裁剪后的推算值高估了约 4.5%；
`ee46dea3` 的提交信息里有同一处笔误。表头三行（4.87 / 91.92 / 96.79 MiB）当时就是对的，
本轮所有数字统一按 1024 进制重算。**提交信息不改写**（那需要 amend/rebase，本轮纪律不允许），
订正记在这里与 fix round 1 的提交信息里。

---

## 14. 崩溃重启策略（Task 2 · DR-T2-4 的处置）

**处置：本任务仍然不做重启，理由与代价在这里写清；不允许以「计划提过、规格没写、代码没有」的形态存在。**

Task 1 登记的理由是「Host 没有真存储，重启没有可恢复的状态语义，而重启时机必须与数据库锁/
迁移事务绑定」。**Task 2 之后这条理由已经变弱**（真 sqlite 与真迁移都在了，sqlite 的事务性也意味着
半个迁移不会留下半提交状态），所以本任务给出一条**更硬、且与实现无关**的理由：

> **已挂载的渲染层无法在不经可见状态机的情况下换端口**（复评 M7 订正过措辞：早先写成
> 「delivered port 不可重建」，字面上过了——端口当然可以新建，壳也随时能再建一个）。
> 真正的约束是：渲染层手里那条 `MessagePort` 是**页面内**建的、由注入桥接到**这个** Host 进程的
> stdin/stdout（Task 0 的路线 1′），而它的**消费点已经固定**——`DatabaseStartupAdmission` 只在
> 「同代 `startupId` + `phase === ready`」时交一次端口，之后渲染层持有的就是那个对象。
> Host 重启会产出一个新的 `ChannelServer` **和一条新端口**；要让渲染层换过去，必须有一条
> **渲染层可见的**状态迁移（新的 `databaseStartupId` + 重新走一次准入），否则旧端口上的所有 RPC
> 静默失败——UI「挂着」而没有任何信号。所以重启必须在**渲染层可见的状态机**里发生
> （`databaseStartupControl` 的 `retry` 语义 + 端口重新投递路径），那是 Task 3/4 与
> 窗口/附件模型一起定的事，不是本进程能单独决定的。
>
> 补充（对**启动期**失败而言，重启的期望收益本来就是 0）：真启动失败是**确定性**的
> （基目录被文件占了、盘符只读、父目录不存在），「重启一次再失败」只是把失败推迟一个来回。

**今天的实际行为（有断言，不是「没人管」）**：

- 启动期失败 → Host 结构化落 stderr 并 `exit(1)` → 壳的 reaper 推 `failed(transport_closed)`，
  渲染层画失败屏（§10.6 的真机现场；`packages/desktop/tests/startupFailure.test.mts`）；
- 就绪**之后**崩 → 同一 reaper 推 `failed(transport_closed)`，且**不会**出现「先 failed 后 ready」
  这种自相矛盾的流（三个监督线程用一个原子量 CAS 决定谁能发布终态）：
  `crates/sidecar-host/tests/host_lifecycle.rs` 的
  `a_host_that_dies_after_ready_publishes_failed_transport_closed`；
- 壳被强杀也不留孤儿（Host 的 stdin EOF 保护 + 壳的 `taskkill /T`），§10.3 的实测。

**交给 Task 3/4 的输入**：`databaseStartupControl` 的 `retry` 语义 + 端口重新投递路径
（谁重新生成 `databaseStartupId`、渲染层如何丢弃旧端口）。

---

## 15. fix round 1 的登记（复评 C1 + I1–I4 + M1–M8）

### 15.1 阶段验收的逐条映射（复评 I3 要求：不许把「半交付」读成「已达成」）

| 阶段验收条目 | Task 2 的状态 |
| --- | --- |
| 1. 真实 `IServiceAccessor` 装配完成，UI 挂载后**至少一个真实业务面可用** | **达成**（40 个真频道；`system.info`/`setting.get` 返回真值、未知命令报 `Method not found`：§10.7；真机 RPC 走通：§10.5） |
| 2. 闸门由**真实** startup 状态驱动；**失败时渲染层收到 `failed` + `errorCode`** | **半交付**：①「闸门由真启动驱动」**达成**（`deferInit` 使第一帧 = 服务面齐备，§1/§10.5）；②「失败**到达**渲染层」**达成**（§10.6 真机 `failed` + `transport_closed`）；③「失败**原因精确到阶段**」**未达成，且是 Task 3/4 的义务不是本任务的成果**——Host 的分段阶段没有离开进程，壳只能判断「就绪前退出」，所以渲染层看到的是粗粒度 `transport_closed`。**不要**把这一条读成已完成（§1 末段、§8.6、§10.6 的差额） |
| 3. 原生依赖在 sidecar 里实测可用 + 自动化用例 | **达成**（§10.4/§12） |
| 4. 没有任何服务因 `listen` 抛错杀死 Host（有断言） | **达成**（§3.1；两层守卫 + 4 个断言 + **合法订阅不被吞**的新断言，见 §15.2） |
| 5. 数据隔离：测试与本地跑不碰真实 `~/.mode`，且无「忘设就指向真实根」的路径 | **达成**（§4：真写入落隔离根 + 真实根的具体路径未被创建；空 base 拒绝在 crate 用例里） |
| 6. sidecar 打包形态落地且**体积实测** | **达成**（§13；闭包口径已按复评 I2 改为真实闭包，并给出下界列） |
| 7. 渲染层未修改；`clippy -D warnings` 零告警、`pnpm verify:pre-push` 通过、新用例落在 CI 入口 | **达成**（§2.4） |
| 8. 重启策略按 DR-T2-4 落地或明确登记 | **达成**（§14，含措辞订正 M7） |

### 15.2 复评 I1：翻译只有一份实现（本轮的**行为改动**）

早先的形态是「映射写了两遍」：`index.ts` 的启动序列把应用阶段**硬编码**成
`"preparing_host_storage"`，而夹具驱动用例测的是 `startupMapping.ts` 的 `applyStorageFrame`
——**绿的是没跑的那条路**。现在的形态：

- `applyStorageFact(fact)` 是**唯一**的翻译实现（`applicationPhaseForDatabaseKind` + 库内相位 +
  迁移事实 + 失败字段都在里面）；真存储回调（生产路径）**直接调它**；
- `applyStorageFrame(frame)` 只是「帧 → 事实」的适配（`storageFactFromFrame`），
  Task 3/4 的 Host → 壳通道落地后由它接帧；
- 新增一条**源码扫描用例**（`packages/desktop/tests/startupMapping.test.mts`）钉住：
  入口必须调 `applyStorageFact`，且**代码里**不得出现 `"preparing_host_storage"` /
  `"preparing_session_storage"` 字面量（注释先剥掉——写这条用例时真被自己的注释触发过一次。
  **该用例做过 RED 验证**：把入口改回硬编码后它当场红，改回即绿）；
- 顺带发现一个真实的类型差并补测：真存储的相位集合比 wire 帧**多一个 `maintaining`**
  （`modeStorageStartupStateSchema` 的枚举里没有它，`DatabaseStartupState.databasePhase` 里有），
  所以「事实」比「帧」宽一格——帧路径永远测不到这一格，只有事实路径能（新用例覆盖）。

### 15.3 复评 I2/M2/M3/M6/M8 的登记

- **I2（体积口径）**：已改成**真实传递闭包**并保留 own-dir 列作显式**下界**，未解析依赖单独列出
  （§13）。旧的实现只量外层目录却在注释里声称「嵌套依赖也算进去」——那是一句与实现不符的话，
  已删。
- **M2（`disk: []` 硬编码）**：`startupMapping.ts` 每个状态都写死 `disk: []`（schema 要求该字段），
  **本轮没有实现磁盘摘要**。风险：将来运行时给出盘面摘要时，仍会被这里丢掉（静默）。已登记为
  Task 3/4 的输入（谁产出 `StartupDiskSummary`、谁负责填）。
- **M3（自验的配置解析档）**：自验现在**覆盖并断言**「资源目录」档（§13），不再靠 env 档的绿灯。
- **M6（两个 `sequence` 计数器）**：Host 的 `StartupStateMachine` 从 0 自增发布；壳的
  `StartupMonitor` 也自己从 0 自增，而渲染层的 `acceptState` 拒绝 `sequence <= state.sequence`。
  **这是一笔未决的所有权**：Task 3/4 加 Host → 壳通道时，必须**只有一个** `sequence` 拥有者
  （要么壳转发 Host 的序号、要么壳按自己的发布重编号并让 Host 的序号只作内部诊断），
  否则合并流会被渲染层当成旧状态丢掉。登记在此，本轮不动（改它就要改壳/传输层）。
- **M8（守卫落点偏离计划）**：计划写的是「修的位置在**壳/传输侧**」，实际落在 **Host 的两个派发边界**
  上。**这是偏离，理由与判据写下来备查**：壳只搬字节、拿不到「哪一次 `listen`」；真正未加护的调用点
  在 `packages/rpc` 的 `onEventListen`（Electron 路径共用，本任务不可改）。落点保住了计划那条的
  **意图**（「不逐个服务打补丁」——守卫在唯一的 `registerChannel` 入口上，一次覆盖全部频道），
  代价（客户端看不到拒绝、要变成 RPC 错误必须改 `packages/rpc`）在 §3.1 写清。
- **补：合法订阅必须仍然工作（复评 I4）**：守卫的「已知代价」正是「一次回归就可能把所有订阅静默
  吞掉」，而此前**所有** `listen` 用例订阅的都是不存在的事件名——那种回归会让套件全绿而渲染层的
  响应式静默死掉。新增断言：订阅 `broadcast` 频道的**真事件** `onMessage`，断言
  `guardedSubscriptions` 与 `dispatchGuards` **都不变**、stderr 无 `listen_guard` 痕迹、
  Host 存活（`apps/desktop-tauri/test/realServices.test.mts`）。
- **补：生产入口的启动失败退出必须无界**（复评 M4）：`process.exit(1)` 原先只挂在
  `stderr.write` 的回调里；回调不触发就等于 Host 挂着等壳的看门狗。现在回调 + **2s 兜底定时器**
  两条路都退出（`index.ts` 的 catch）。


