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

## 0. 本任务做到哪、没做到哪（先读这条）

**做到**：Tauri 壳能拉起一个 Node **Host sidecar**，把渲染层要的两样东西送进 webview——
一个承载 `ChannelServer` 的真 `MessagePort`，以及一条 database-startup 状态流——
于是渲染层**未改动**的启动闸门会放行（`phase === "ready"` 且两端 `startupId` 同代）。

**没做到（Task 2 的范围）**：服务面全是 **stub**。UI 会挂载，但几乎所有交互都会失败——
这是**预期的中间态**（DR-T1-1：本任务的「成功」定义是闸门放行，不是服务可用）。
不要把这时的「UI 能显示」读成「能发布」。

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
| Host sidecar | `packages/desktop/src/host-standalone/**`（Node，入口 `index.ts`） | 真 `ChannelServer` + 服务频道（Task 1 为 stub）；数据根解析；帧形状校验 | 不建窗口、不碰 Tauri |
| 进程/传输库 | `crates/sidecar-host/**` | spawn/监督/帧编解码/桥脚本/启动状态/路径挑选纯函数 | **不引用任何 Tauri 类型**（理由见 §6） |

**startupId 的所有权**：壳生成、经环境变量 `MODE_HOST_STARTUP_ID` 交给 Host，并烘进注入桥。
「端口负载的 `databaseStartupId`」与「状态流的 `startupId`」由此天然同代——这正是闸门的判据。

**发布状态的所有权（Task 1 的口径）**：**壳是渲染层看到的 `DatabaseStartupState` 的唯一发布者**
（`crates/sidecar-host/src/startup.rs` 的 `StartupMonitor`）。它合并两类事实：

- Host 进程事实（壳直接观测）：就绪判据是 **Host 的第一帧**（`ChannelServer` 的 `Initialize`）；
  失败判据是起不来 / 就绪前退出 / 就绪期限内没有第一帧 / 就绪后崩；
- Task 2 起，Host 报告的分段阶段（`preparing_host_storage` 等）要经壳**合并**进同一份状态再发布。

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

`failedPhase` 一律记 `starting`（Task 1 只有「拉起 Host」这一段）。三个监督线程用一个原子量
CAS 决定**谁**能发布终态，所以不会出现「先 failed 后 ready」这种自相矛盾的流。

### 3.1 stub 的失败形态，以及一条**会杀死 Host** 的路径（复评 C1）

- `call(command)`：显式拒绝，文案带频道名与命令名，错误对象带 `code: "not_implemented"`。
  **绝不**返回 `undefined` 或空对象——那会让调用方把「没实现」读成「成功但没数据」。
- `listen(event)`：**不能抛错**。`ChannelServer.onEventListen`（`packages/rpc/src/channelServer.ts`）
  调 `channel.listen(...)` 时**没有 try/catch**（对照 `onPromise` 会
  `try { channel.call } catch { Promise.reject }`）。stub 在 `listen` 里抛错的实测后果：
  异常穿出 `onRawMessage` → `Emitter.fire` → stdin 的 `data` 处理函数，**Host 进程当场死亡**，
  同一端口上的在途 RPC 全部变成 `transport_closed`；而渲染层一挂载就会订阅服务事件，
  于是「闸门放行后几秒 Host 就死」。stub 因此返回一个**永不发事件的 Event**（不假装有事件），
  同时**不静默**：第一次订阅在 stderr 留一条 `not_implemented` 记录，累计次数在
  `host.describe` 的 `stubEventSubscriptions` 里可观察。
- **危险面的通用表述（Task 2 必读）**：**任何服务只要在 `listen` 里抛错，就会杀死 Host**——
  壳/传输侧对 channel dispatch 没有守卫。Task 2 接真服务时若发现真实现会抛，
  修的位置是**壳/传输侧**（给 dispatch 加守卫并把它变成 RPC 错误），不是逐个服务打补丁；
  在那之前，每个 `IServerChannel` 实现都得自己保证 `listen` 不抛。

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
- **Task 1 的 Host 不写盘**：没有任何存储实现（`grep node:fs packages/desktop/src/host-standalone`
  零命中），数据根只用于隔离与上报；用例断言运行后隔离目录里**连基目录都不出现**。
- 测试与本地跑一律 `MODE_DATA_BASE_DIR` + `HOME`/`USERPROFILE` **双覆写**，并**对隔离生效做断言**
  （`apps/desktop-tauri/test/portRoundTrip.test.mts` 断言两条覆写各自生效、根等于 `{base}/.mode`，
  且不产生任何目录）。

## 5. 覆盖分工（哪些自动化、哪些必须人工）

| 层 | 覆盖方式 | 进 CI |
| --- | --- | --- |
| Host 的进程/监督/帧/启动状态 | `cargo test -p sidecar-host`（15 单测 + 9 集成；**真拉 Node 子进程**） | **是**（Windows 腿，release.yml） |
| 跨语言帧边界（Rust 编码 ↔ Node 解码） | `the_shell_frame_path_reaches_the_host_and_gets_a_reply`：Rust 写一帧真请求、读回真应答 | **是**（同上） |
| 端口往返（真实产品 Host） | `apps/desktop-tauri/test/portRoundTrip.test.mts`：真 Host + 真 `MessagePort` + 真 `ChannelClient`；含「订阅事件不杀 Host」「坏帧丢弃」「数据隔离」 | **是**（verify job，release.yml） |
| 纯端口路径回归（Task 0 的） | `test/portMessagePath.test.mts`、`test/hostChannelStdio.test.mts` | **是**（同上） |
| 跨语言字面量（频道名/命令名/状态词汇） | `packages/desktop/tests/host-standalone.test.mts` | **是**（verify job） |
| **真 webview 的注入/转移/ACL/时序** | `apps/desktop-tauri/test/webviewRoundTrip.test.mts`（**opt-in，默认 skip**） | **不能** |
| `mode_tauri::host::{resolve_config, resolve_entry}`（壳侧的候选枚举、家目录、startupId 生成） | **无自动化覆盖**：要测它们就得把 `AppHandle`/具体 runtime 带进测试二进制（§6），只在 opt-in webview 跑里被走到 | 否（**登记在案的缺口**） |
| `host_send`/`host_recv`/`host_startup_state` 的 Tauri 命令**包装层** | 命令体本身没有自动化（本仓不引入 `tauri::test` webview mock 运行时，M1 已定）；其**逻辑**（`send_frame`/`next_frame`/`wait_for_startup_change`/`to_state_json`）由 crate 用例覆盖 | 否（**登记在案的缺口**） |
| 测试 harness 页面（`test/harness/**`）的**类型检查** | 不在任何 tsc 工程里（vite alias 解析到浏览器 shim 与真实渲染层源码，用 tsc 解析会把 node 侧 barrel 拉进 DOM 工程）；构建与运行由 vite/tsx 负责 | 否（**登记在案的缺口**） |

**崩溃重启策略：本任务刻意不做（复评 M11 要求登记）**。理由：Task 1 的 Host 没有真存储，
「重启」没有可恢复的状态语义；而 Task 2 一旦接上 `node:sqlite`，重启时机必须与
数据库锁/迁移事务的语义绑定（半个迁移没提交时重启比失败更糟）。DR-T1-3 的判据是
「不留孤儿 + 失败要能被推给渲染层」，两条都已由本任务达成；重启策略登记为**Task 2 的输入**，
届时连同 `databaseStartupControl` 的 `retry` 语义一起定。

### 发版前人工跑一次（发布流程上的硬要求）

```bash
MODE_TAURI_WEBVIEW_ROUNDTRIP=1 node --import tsx --test apps/desktop-tauri/test/webviewRoundTrip.test.mts
```

判据（自动断言）：`status: "ok"`、`gateOpened: true`、`portIsRealMessagePort: true`、
`initializeObserved: true`、`schemaAccepted: true`、`statePhase: "ready"`、
`startupIdMatches: true`、`describe.dataBaseDir` 等于临时基目录、`describe.dataRoot` 等于
`{base}/.mode`、进程表里不残留 Host。
**漏跑这条 = 端口路径悄悄坏掉而发布流程说全绿**——这正是本仓库反复吃亏的那类失败（DR-T1-2 的代价）。

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
3. **打包形态未验**：资源目录里的 sidecar 落点、随包的 Node 运行时、体积（Task 3/5）。
4. **macOS / Linux 不在本阶段范围**（Ruling 20）：`taskkill /T` 是 Windows 的收树手段，
   别的平台上靠 Host 的 stdin EOF 保护兜底，未在本阶段验证。
5. **没有重启策略**（§5 已登记理由）。

## 9. 纪律登记（clean-room）

- `crates/sidecar-host/**`、`apps/desktop-tauri/**` 全部新写；
- 上游衍生文件里**只读接口面**：`packages/rpc` 的导出签名与类型声明（`MessagePortLike`、
  `ChannelServer` 的构造与 `ready()`、`IServerChannel`、`Event`/`toDisposable`、`VSBuffer`、
  `BufferReader`/`deserialize`）、`packages/shared` 的常量与 zod schema（`InternalChannels`、
  `ServiceChannels`、`databaseStartupStateSchema` 及其错误码/阶段枚举）、
  `packages/services/src/paths.ts` 的**数据根口径**（`getDataBaseDir`/`getModeDataRootDir`）、
  `packages/desktop/src/renderer/src/databaseStartupAdmission.ts`（**作为未改动源码被集成测试与
  验收台 import**，不读实现体做重写）；
- 复评引用到的一处上游实现细节（`ChannelServer.onEventListen` 没有 try/catch）是**复评给出**
  并由我用真子进程复现的（§10 的 C1 现场），不是照抄实现；
- 帧 envelope 的形状（首字节是序列化层标签而非 `RequestType`）是用 `@mode/rpc` 的公开
  反序列化面**跑出来**的（`deserialize` 真客户端帧 → `[100, 0, "host", "describe"]`）；
- 渲染层零改动：`git diff` 对 `packages/desktop/src/renderer/**` 为空。

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
