# Spec：Tauri 壳的 Host 契约（M3 Task 1）

前置：阶段计划 `docs/plans/2026-10-16-tauri-m3-ecosystem.md`（DR-M3-1/2/3）、
任务子计划 `docs/plans/2026-10-16-m3-host-sidecar.md`（DR-T1-1/2/3）、
Task 0 spike `docs/plans/2026-10-16-m3-port-transport-spike.md`（§3/§4 是本契约的路线来源）、
M1 的挂载缺口表 `docs/specs/tauri-platform-commands.md` §4.2。
纪律：`docs/specs/clean-room-policy.md`——`crates/**` 与 `apps/desktop-tauri/**` 全部适用；
`packages/desktop/src/host-standalone/**` 是新写文件，未被上游衍生文件「污染」。

## 0. 本任务做到哪、没做到哪（先读这条）

**做到**：Tauri 壳能拉起一个 Node **Host sidecar**，把渲染层要的两样东西送进 webview——
一个承载 `ChannelServer` 的真 `MessagePort`，以及一条 database-startup 状态流——
于是渲染层**未改动**的启动闸门会放行（`phase === "ready"` 且两端 `startupId` 同代）。

**没做到（Task 2 的范围）**：服务面全是 **stub**。UI 会挂载，但几乎所有交互都会失败——
这是**预期的中间态**（DR-T1-1：本任务的「成功」定义是闸门放行，不是服务可用）。
不要把这时的「UI 能显示」读成「能发布」；也不要让 stub 的失败形态含糊到被读成「成功但没数据」，
那是本任务明令禁止的失败模式（§3）。

**仍然未验**：真实 renderer 挂载（Task 3 换 `frontendDist`）、`node:sqlite`/`node-pty` 在
sidecar 里可用（Task 2）、打包形态的 sidecar 落点与体积（Task 3/5）。

## 1. 组件与所有权

| 组件 | 位置 | 职责 | 不做什么 |
| --- | --- | --- | --- |
| Rust 壳 | `apps/desktop-tauri/src-tauri`（`main.rs`、`host.rs`、`window.rs`） | 建窗口 + 注入桥；spawn/监督 Host；两个 raw 帧命令 + 一个状态长轮询；退出时收 Host | 不持业务状态（`AGENTS.md` 的「进程、协议与远程控制」） |
| Host sidecar | `packages/desktop/src/host-standalone/**`（Node，入口 `index.ts`） | 真 `ChannelServer` + 服务频道（Task 1 为 stub）；数据根解析 | 不建窗口、不碰 Tauri |
| 进程/传输库 | `crates/sidecar-host/**` | spawn/监督/帧编解码/桥脚本/启动状态 | **不引用任何 Tauri 类型**（理由见 §6） |

**startupId 的所有权**：壳生成、经环境变量 `MODE_HOST_STARTUP_ID` 交给 Host，并烘进注入桥。
「端口负载的 `databaseStartupId`」与「状态流的 `startupId`」由此天然同代——这正是闸门的判据。

**发布状态的所有权（Task 1 的口径）**：**壳是渲染层看到的 `DatabaseStartupState` 的唯一发布者**
（`crates/sidecar-host/src/startup.rs` 的 `StartupMonitor`）。它合并两类事实：

- Host 进程事实（壳直接观测）：就绪判据是 **Host 的第一帧**（`ChannelServer` 的 `Initialize`）；
  失败判据是起不来 / 就绪前退出 / 就绪期限内没有第一帧 / 就绪后崩；
- Task 2 起，Host 报告的分段阶段（`preparing_host_storage` 等）要经壳**合并**进同一份状态再发布。

为什么不让渲染层同时听两个发布者：闸门的判据是「同代匹配」，两个发布者会让「同代」的语义分裂。

## 2. 环境契约（壳 → Host）

| 变量 | 方向 | 含义 |
| --- | --- | --- |
| `MODE_HOST_STARTUP_ID` | 壳 → Host | 本次实例的启动代 id（`[A-Za-z0-9_-]`） |
| `MODE_DATA_BASE_DIR` | 壳 → Host | 数据根覆写（隔离用；见 §5） |
| `HOME` / `USERPROFILE` | 壳 → Host | 家目录覆写（数据根派生用） |
| `MODE_HOST_SIDECAR_ENTRY` | 进程环境 → 壳 | 入口脚本显式路径（开发/测试用；打包形态走资源目录） |
| `MODE_HOST_NODE` | 进程环境 → 壳 | Node 可执行文件（默认 PATH 上的 `node`） |
| `MODE_HOST_SIDECAR_CWD` | 进程环境 → 壳 | Host 工作目录（`--import tsx` 的裸模块解析从它开始） |

入口解析顺序：`MODE_HOST_SIDECAR_ENTRY` → 资源目录 `<resource>/host-sidecar/index.{mjs,ts}` →
（debug 构建）仓库相对路径 `packages/desktop/src/host-standalone/index.ts`。**不硬编码绝对路径**：
仓库相对那档只在 debug 构建存在（release 下编译器不会编进去），打包落点是 Task 3/5 的事。

## 3. 两个方向的三条命令与失败语义

命令名（与 Task 0 spike 的 `spike_host_send`/`spike_host_recv` **同一对语义**，去掉 `spike_` 前缀）：

| 命令 | 方向 | 形态 |
| --- | --- | --- |
| `host_send` | 页面 → 壳 → Host | raw body（`InvokeBody::Raw`），整帧写进 Host stdin |
| `host_recv` | Host → 壳 → 页面 | 长轮询，返回 `tauri::ipc::Response`（raw bytes；页面上是 **`ArrayBuffer`**） |
| `host_startup_state` | 壳 → 页面 | 长轮询（`{ since }`），返回 `databaseStartupStateSchema` 形状的状态对象 |

**三条硬约束**（都有失败现场，见 spike §4）：

1. **端口只能在页面内建**：`tauri 2.12.4`/`wry 0.57.0` 没有 port/transferable 能力。桥在页面里
   `new MessageChannel()`，`port1` 用 `window.postMessage(payload, "*", [port1])` 交给渲染层——
   渲染层拿到的是**真 `MessagePort`**，且 `event.source === window`（`dispatchEvent` 做不到这条）。
2. **入方向必须包 `Uint8Array`**：`MessagePortProtocol` 的收帧判定是 `e.data instanceof Uint8Array`，
   而 Tauri 的 raw 响应是 `ArrayBuffer`；直接投会被**静默丢弃**（RPC 挂到超时，页面看不出原因）。
3. **投递时序是语义**：桥只在 `document.readyState !== "loading"` 之后投递（渲染层的
   `addEventListener("message", …)` 注册在模块脚本里，提前投会白丢一次）。

**失败语义**（DR-T1-3）：sidecar 崩溃必须由壳**主动推** `phase: "failed"` + `errorCode`，
不能让渲染层等到 40s 失败屏——超时是兜底，不是主路径。映射（`StartupErrorCode`）：

| 现场 | `errorCode` | 触发点 |
| --- | --- | --- |
| 拉不起进程（node/入口缺失、不可执行） | `open_failed` | `HostHandle::spawn` 返回 `Err` → 壳构造「已失败」句柄 |
| 就绪前退出 / 传输断开（含就绪后崩） | `transport_closed` | reader 读 EOF / reaper 收到退出 |
| 就绪期限内没有第一帧 | `startup_status_timeout` | 看门狗（默认 20s） |
| 配置非法（既无入口也无 node 参数） | `unsupported_runtime` | `HostHandle::spawn` 校验 |

`failedPhase` 一律记 `starting`（Task 1 只有「拉起 Host」这一段）。三个线程用一个原子量
CAS 决定**谁**能发布终态，所以不会出现「先 failed 后 ready」这种自相矛盾的流。

**stub 的失败形态（DR-T1-1）**：每个服务频道的每个命令都拒绝，错误文案带**频道名与命令名**，
并带 `code: "not_implemented"`。**绝不**返回 `undefined` 或空对象——
那会让调用方把「没实现」读成「成功但没数据」。`listen` 同样显式抛错，不给「订阅成功但永不发事件」的假象。

## 4. 数据隔离

- 壳：`MODE_DATA_BASE_DIR` → 否则 `<home>/.mode`（`crates/sidecar-host/src/lib.rs` 的 `data_root_from`）；
- Host：同一条口径（`packages/desktop/src/host-standalone/dataRoot.ts`），报在 `host.describe` 里；
- **Task 1 的 Host 不写盘**：没有任何存储实现，数据根只用于隔离与上报；
- 测试与本地跑一律 `MODE_DATA_BASE_DIR` + `HOME`/`USERPROFILE` **双覆写**，并**对隔离生效做断言**
  （`apps/desktop-tauri/test/portRoundTrip.test.mts` 断言两条覆写各自生效，且跑完隔离目录里
  不出现 `.mode`）。

## 5. 覆盖分工（哪些自动化、哪些必须人工）

| 层 | 覆盖方式 | 进 CI |
| --- | --- | --- |
| Host 的进程/监督/帧/启动状态 | `cargo test -p sidecar-host`（13 单测 + 5 集成；**真拉 Node 子进程**） | 本地必跑；CI 目前未纳入该 `-p` 列表（见 §8） |
| 端口往返（Node 侧等价物） | `apps/desktop-tauri/test/portRoundTrip.test.mts`：真 Host + 真 `MessagePort` + 真 `ChannelClient` | 是（需 `pnpm --filter @mode/desktop-tauri test`，见 §8） |
| 纯端口路径回归（Task 0 的） | `test/portMessagePath.test.mts`、`test/hostChannelStdio.test.mts` | 同上 |
| 跨语言字面量（频道名/命令名/状态词汇） | `packages/desktop/tests/host-standalone.test.mts` | **是**（发布流程 verify 已跑 `packages/desktop/tests/*.test.mts`） |
| **真 webview 的注入/转移/ACL/时序** | `apps/desktop-tauri/test/webviewRoundTrip.test.mts`（**opt-in，默认 skip**） | **不能** |

### 发版前人工跑一次（发布流程上的硬要求）

```bash
cd apps/desktop-tauri    # 或任意工作目录：用例自己会构建页面与 bin
MODE_TAURI_WEBVIEW_ROUNDTRIP=1 node --import tsx --test apps/desktop-tauri/test/webviewRoundTrip.test.mts
```

判据（自动断言）：`status: "ok"`、`gateOpened: true`、`portIsRealMessagePort: true`、
`initializeObserved: true`、`schemaAccepted: true`、`statePhase: "ready"`、
`startupIdMatches: true`、`describe.dataRoot` 等于临时数据根、进程表里不残留 Host。
**漏跑这条 = 端口路径悄悄坏掉而发布流程说全绿**——这正是本仓库反复吃亏的那类失败（DR-T1-2 的代价）。

### 人工可观察的开发形态

`node scripts/rust-cargo.ts run -p mode-tauri`（debug）会起真壳：窗口里是 M1 的**命令面验收台**，
底部多了「Host 通道（M3 Task 1）」一屏，显示注入桥的 startupId、端口是否送达、是否真 `MessagePort`、
状态流的 phase/sequence/errorCode，以及端口上旁听到的帧。这一屏是**开发期诊断面**，
Task 3 把 `frontendDist` 换成真 renderer 后仍保留（验收台是命令面回归工具，不删）。

## 6. 为什么 `crates/sidecar-host` 里没有 Tauri 类型

只要该 crate 引用**任何** Tauri 类型，链接面就会把 `wry`/`webview2` 拉进来，而 **cargo 的测试目标
没有应用清单**（`tauri-build` 只给 bin 注入）——Windows 上测试二进制会静态导入 `comctl32` +
`WebView2Loader.dll`，`cargo test` 直接以 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` 起不来。

这是**实测**过的：本 crate 的第一版把窗口装配与 `#[tauri::command]` 放在里面，
测试二进制导入表里同时出现 `comctl32.dll` 与 `WebView2Loader.dll`，进程起不来（连 13 条纯逻辑用例
都跑不了）。所以分工是：**进程/传输/桥脚本在 `crates/sidecar-host`（框架无关、用例能真跑）**，
**窗口与命令面在壳 crate**（只有 bin 目标，不需要自己的应用清单）。

## 7. 与既有语义的关系（不许绕过的东西）

- 每窗口一个 window-scoped Local Host：本任务是**一个** Host（单窗口），窗口模型与附件调度是 Task 3。
- 手机远控 / 远程 workspace：本任务不涉及；`IServiceAccessor` 的 stub 意味着任何远端语义也还没接。
- 渲染层零改动：契约的全部价值就是**不需要改渲染层**。若将来发现必须改，那是范围变更（§9）。

## 8. 已知边界与 CI 缺口（写给下一个任务）

1. **CI 未跑这两个包的用例**：发布流程的 `verify` job 跑的是
   `packages/desktop/tests/*.test.mts`（已覆盖本任务的跨语言契约用例），但**没有**跑
   `pnpm --filter @mode/desktop-tauri test`，Windows 腿的 Rust 步骤只跑
   `cargo test -p platform -p protocol -p updater`（**不含** `-p sidecar-host`）。
   因此本任务的 Rust 用例与端口往返用例目前是「本地必跑、CI 未跑」。
   把两者加进 CI 是独立的小改动（`.github/workflows/release.yml`），本次未改以免动发布流水线——
   **这是登记在案的缺口，不是「已经覆盖」**。
2. **桥的 `host_recv` 长轮询是无界队列+阻塞背压**：帧队列默认 256 帧，满了读线程阻塞
   （真背压，不丢帧）。`MessagePortFlowControl` 那一层没有用（Task 0 spike §5 的未验证项 3）。
3. **Tauri `Channel<Response>` 的 raw 推送路径未实测**（Task 0 spike §5 的未验证项 5）——
   本任务没用到它。
4. **打包形态未验**：资源目录里的 sidecar 落点、随包的 Node 运行时、体积（Task 3/5）。
5. **macOS / Linux 不在本阶段范围**（Ruling 20）：`taskkill /T` 是 Windows 的收树手段，
   别的平台上靠 Host 的 stdin EOF 保护兜底，未在本阶段验证。

## 9. 纪律登记（clean-room）

- `crates/sidecar-host/**`、`apps/desktop-tauri/**` 全部新写；
- 上游衍生文件里**只读接口面**：`packages/rpc` 的导出签名与类型声明（`MessagePortLike`、
  `ChannelServer` 构造与 `ready()`、`VSBuffer`、`BufferReader`/`deserialize`、`IServerChannel` 形状）、
  `packages/shared` 的常量与 zod schema（`InternalChannels`、`ServiceChannels`、
  `databaseStartupStateSchema`、`databaseStartupErrorCodeSchema`、`databaseStartupPhaseSchema`）、
  `packages/desktop/src/renderer/src/databaseStartupAdmission.ts`（**作为未改动源码被集成测试与
  验收台 import**，不读实现体做重写）；
- 本次没有读任何函数体来「照抄实现」。唯一一次按实测反推协议形状的是**帧 envelope**
  （首字节是序列化层标签而非 RequestType）——那是用 `@mode/rpc` 的公开反序列化面**跑出来**的
  （`deserialize` 一个真客户端帧 → `[100, 0, "host", "describe"]`），不是读实现体得来的，
  且判定逻辑写进了 `crates/sidecar-host` 的注释与用例；
- 渲染层零改动：`git diff` 对 `packages/desktop/src/renderer/**` 为空。
