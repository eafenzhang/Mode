# M4b：Host 并入壳（2026-10-29 立项）

> **2026-10-29 决策升级**：本轮起改为「能 Rust 化的全部重写，工程量不作约束」，
> 上位计划见 `docs/plans/2026-10-29-full-rust-rewrite.md`。本文件仍是 Host 并入壳的
> **机制子计划**（线格式、路由、所有权），对应总计划的 W0–W3；其中 IM 通道与 SSH 不再
> 「取决于产品裁决」，分别进 W5（Rust 自实现四通道）与 W3-9（russh）。
>
> 上游依据：`docs/plans/2026-10-09-tauri-rewrite-roadmap.md` §M4b。
> 本轮已落地的第一块：`docs/specs/rpc-wire-format.md` + `crates/rpc-wire`
> （RPC 线格式 Rust 侧 + 与 TS 的字节级黄金对拍）。
>
> **结论先行**：M4b 不是「一次改动」，而是把 Host 的服务面从 Node 子进程搬进壳的
> tokio 运行时。路线图写的「2–4 周」低估了体量——服务面是 `packages/services`（约
> 109k 行）经 `packages/shared/src/channels.ts`（517 个方法签名）暴露的频道集合。
> 本文件把它拆成可独立验收的六个增量，并**先核验了收益前提**。

## 1. 收益前提核验（2026-10-29 实测，win32-x64）

「砍掉 35 MB」需要先说清这 35 MB 是什么。发运集里 `host-sidecar/` 实测 **31.60 MiB**
（2026-10-23 `measure:ship-core-payload`：host-sidecar 31.60 + agent 19.49 + tools 8.50 +
config 0.19）。本轮把 externals 闭包按 `host-sidecar-ship.ts` 的同一套剪枝规则量了一遍：

| 组成 | 未剪枝 | 剪枝后（随包） | M4b 后是否消失 |
| --- | --- | --- | --- |
| `node-pty` | 62.58 MiB（全平台 prebuilds） | 7.14 MiB | **会**：终端 PTY 已下沉到壳（`terminal-pty-in-shell.md`），Host 只服务 Web/远程旧路径 |
| `@larksuiteoapi/node-sdk` | 24.33 MiB | 9.24 MiB | **取决于 IM 方案**：Rust 重写飞书通道才会消失，否则仍需随包或按需资产 |
| `node-forge` | 1.57 MiB | 1.57 MiB | **会**：凭据加密改 `ring` |
| `protobufjs`（飞书依赖） | 2.81 MiB | 1.15 MiB | 同上，跟随飞书 |
| `ssh2` | 1.07 MiB | 1.07 MiB | **不会**：LR-4 明确保留 SSH 后端（`@mode/server/remote`），搬走要先有 Rust SSH 客户端 |
| `undici` | 1.13 MiB | 1.05 MiB | **会**：provider 请求改 `reqwest` |
| `yaml` / `wecom` / 其余 | — | ≈2.5 MiB | 视各域迁移 |
| Host bundle（`index.mjs`） | — | ≈5.5 MiB | **会** |
| **合计** | 102.61 MiB / 66 包 | **26.12 MiB** | 净减 **≈15–17 MiB**，另有 ≈10.4 MiB 卡在 IM/SSH 决策上 |

结论：

1. **「砍掉 35 MB」不是终态口径**。全量 Rust 化之后没有 Node 宿主，
   `node-pty`/`node-forge`/`undici`/`ssh2`/飞书/企微/protobufjs 全部由 Rust 实现取代，
   不存在「能砍多少」的问题；对外只承诺「核心发运集不含 Node 运行时」。
   下表只是**中间状态**的字节归因，用来判断哪些域迁完可以立刻摘掉哪些包。
2. **T_ready 的 200–400 ms 仍是估计**。现有 `measure:cold-start` 只给总时长
   （2026-10-23 基线 825 ms），没有「Node boot / 存储 / 服务装配」的分解，M4b-0
   要补这条分解，否则改完无法归因。
3. **字节释放跟随域迁移**：先迁移不依赖外部协议的域（settings、file、task-index、
   credential、terminal），把 node-pty/node-forge/undici 的字节真正摘掉；IM 与 SSH
   按 W5/W3-9 的 Rust 实现推进，不再作为「放弃项」。

## 2. 目标与边界

- 目标：renderer ↔ Host 的 RPC 不再经过「Node 子进程 + stdio 帧」；服务面由壳直接持有，
  终态 `host-sidecar/` 目录从发运集消失，`T_ready` ≤ 500 ms（同口径复测）。
- 不在本文件：Agent 运行时退出（总计划 W4）与生态面替换（W5，IM/TUI/server）——
  它们同属全量 Rust 化，但机制与本文件的线格式/路由无关。
- 不变的所有者：Host 进程事实（重生/退避）仍归壳 `HostSupervisor`；迁移期间 Node Host
  仍是未被迁移频道的唯一所有者，**同一频道同一时刻只能有一个所有者**。

## 3. 增量拆分

### M4b-0 线格式与度量（本轮完成一部分）

- [x] `docs/specs/rpc-wire-format.md`：把 `@mode/rpc` Layer 1 的字节形态写成契约
      （7 个 tag、VQL、分派顺序、负数与超出 int32 数字的行为、Object 的 JSON 回退）。
- [x] `crates/rpc-wire`：Rust 侧 `serialize`/`deserialize`，28 条黄金样例与 TS 实现
      逐字节对拍（`cargo test -p rpc-wire` 8 条：6 单元 + 2 对拍）。
- [ ] `measure:host-startup`：在既有 `measure:cold-start` 之外补分解（Node boot / 数据根
      打开 / 服务装配 / 首帧）。**这是 M4b 的归因尺子，先于任何搬迁**。
- 退出条件：线格式对拍常绿；启动分解有基线。

### M4b-1 协议层 Rust 侧

- 范围：请求/响应/事件/cancel 的消息头与 `RequestType` 编号，帧的 4 字节大端长度前缀，
  以及 scoped attachment 头的解析边界——只做编解码，不做频道。
- 对拍：与 TS `protocol.ts`/`ipc.ts` 的行为对齐，同样用黄金字节；不读上游实现。
- 退出条件：一条真实 RPC 请求/响应在 Rust 侧编解码后与 TS 字节一致（含错误响应）。
- **2026-10-29 完成（除 scoped 头）**：`crates/rpc-wire` 新增 `frame`（13 字节头，未知
  type 原样保留）、`stdio`（4 字节前缀 + 分片/粘包 + 64 MiB 上限与「只报一次」）、
  `envelope`（四种请求 + 五种回帧 + `is_rpc_request_frame`/`is_rpc_response_frame`/
  `decode_rpc_frame`）。14 条黄金样例（4 消息头、2 stdio、5 请求信封、5 回帧）逐字节对拍；
  `cargo test -p rpc-wire` 27 条通过。scoped attachment 头按 spec 非目标，留给
  M4b-3 的路由前处理。

### M4b-2 壳内 ChannelServer

- 范围：频道注册、方法分派、事件订阅/取消、错误与取消的映射、并发与顺序保证
  （同一频道内按到达顺序处理；不允许为了吞吐打乱顺序）。
- 形态：纯库（`crates/rpc-host`），不接产品，可在 `cargo test` 里用假频道跑通。
- 退出条件：与 TS `ChannelServer` 的行为对拍（同输入 → 同输出帧序），含取消与事件。

### M4b-3 帧路由（灰度开关）

```
renderer（WebView）
   │  Tauri event / MessagePort ←→ 壳 host bridge（现有）
   ▼
壳 frame router（新增）
   ├─ 频道属于 Rust 服务面 → rpc-host（进程内）
   └─ 其余频道            → Node sidecar stdin（现状，逐字节不变）
```

- 开关：`MODE_HOST_INPROCESS_CHANNELS=<逗号分隔频道>`，**启动期一次性决定**，
  不做运行时热切（避免双所有者与事件乱序）。
- 顺序：路由在 scoped 解复用**之后**按 channel 名分流；同一频道的事件与响应必须走同一条
  下行路径，否则渲染层会看到乱序。
- 退出条件：开关为空时行为与今天完全一致（字节级回归）；打开一个只读频道
  （`host` 诊断面）时 UI 正常。

### M4b-4 逐域迁移（按风险从低到高）

| 顺序 | 域 | 频道 | 依赖 | 摘掉的字节 |
| --- | --- | --- | --- | --- |
| 1 | 设置读取 | `setting` | 无（壳已读 settings.json） | 0（先证明路由） |
| 2 | 文件操作 | `file` / `fs` | 无 | 0 |
| 3 | 任务索引 | 经 `file`/`task` 侧面 | `rusqlite`（`agent-core` 已有） | 0 |
| 4 | 凭据 | `credential` | `ring` 替换 `node-forge` | 1.57 MiB |
| 5 | 终端旧路径 | `terminal` | 壳已持有 PTY | 7.14 MiB |
| 6 | Git | `git` | 需 `gitoxide` 或调用系统 git | 0 |
| 7 | provider / 模型 | `provider` / `model` | `reqwest` 替换 `undici` | ~1.05 MiB |
| 8 | IM 通道 | `bots` | Rust 自实现四通道（总计划 W5；不再复用 SDK） | 9.24–10.4 MiB |
| 9 | SSH/WSL/Docker | `remote` | `russh` + 本地进程（总计划 W3-9） | 1.07 MiB |

每域完成的定义（缺一不可）：该频道的 TS 实现保留为 oracle；对拍矩阵覆盖正常/错误/取消/
事件四类；`MODE_HOST_INPROCESS_CHANNELS` 打开后该域的真机用例通过；关闭开关仍走 TS 路径。

### M4b-5 默认切换与删装配

- 默认值翻到壳内；删 `host-standalone` 的 stdio 帧装配、`stdioFrames`/`stdoutGuard`
  与 `host-sidecar/` 发运集；`measure:ship-core-payload` 与 `measure:cold-start` 同口径复测。
- 退出条件（整阶段）：`T_ready` ≤ 500 ms；`host-sidecar/` 从发运集消失；
  两条链路语义（`desktop-continuous` / `web-remote-replayable`）不变；
  20 个真实模型任务的验收口径不降低（DR-07 的硬前置不变）。

## 4. 工作量

**工程量不作为约束**（2026-10-29 决策，见总计划 DR-08）。本文件只固定顺序与验收：
W0 地基 → W1 平台基元 → W2 传输切换 → W3 逐域迁移 → W6 删除编排。
每个域的完成定义与总计划一致（四类对拍 + 真机 + 开关回滚），不做「按成本裁剪」的取舍。

## 5. 风险与对冲

| 风险 | 对冲 |
| --- | --- |
| 同一频道两个所有者（双写） | 启动期一次性开关；运行时不允许热切；每个域迁移完才翻默认值 |
| 事件与响应乱序 | 路由按频道绑定同一条下行路径；按到达顺序处理，不为吞吐并发化 |
| scoped attachment / 远程 scope 被拆坏 | 路由发生在 scope 解复用之后；先补 scope 级对拍再动 |
| Rust 编解码与 TS 微差（负数、浮点、嵌套二进制） | 黄金字节对拍进 CI；Object 分支的嵌套 `Uint8Array` 还原尚未实现，见 §6 |
| 中间状态的字节口径被误当成目标 | §1 的字节归因只用于判断「哪个域迁完能摘哪个包」；唯一承诺是「核心发运集不含 Node 运行时」 |
| 迁移期回归发现晚 | 每域必须保留 TS oracle 与开关回滚；真机用例在每域出口跑 |

## 6. 已知差距（本轮未做）

- **嵌套 `Uint8Array` 的还原**：Rust 侧 `Value::Json` 原样承载 JSON 文本，不解
  `__mode_rpc_nested_uint8array_v1` 标记。在补齐前，Rust 侧不得承接含二进制附件的消息
  （skill sync、附件传输）。
- **协议层（M4b-1）与帧长度前缀**尚未实现。
- **启动分解基线**未测（M4b-0 未完成项）。
- 本轮没有改变任何产品路径：Node Host 仍是唯一所有者，行为与之前一致。
