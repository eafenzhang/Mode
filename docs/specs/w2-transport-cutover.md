# W2 传输切换收口（帧层流控裁决 / 默认切换语义 / 退场条件）

> 状态：v1（2026-11-20）。上位计划：`docs/plans/2026-10-29-full-rust-rewrite.md` §W2、
> `docs/plans/2026-10-29-m4b-host-into-shell.md`（机制与逐域迁移）。
> 本文是对勘察结论（2026-11-20）的三项裁决与验收清单，替代计划里「流控（saturated/drained）
> 与 scoped attachment 逐项对拍」这句的模糊表述。

## 1. 链路现状（勘察结论，file:line 见括号）

```text
renderer（WebView）
 │ 页面内 MessageChannel（注入桥 bridge.ts 转交）           bridge.ts:88-96
 ▼
Tauri host_send/host_recv（长轮询）                         host.rs:409-444
 │ InprocessState::decide 三路：Local / Dropped / Forward   inprocess.rs:182-205
 ├─ Local  → 壳内 ChannelServer（base + scope 复用）        inprocess.rs:229-279
 └─ Forward/Control → Node Host sidecar stdin（逐字节）     host.rs:419-430
      ▼
host-standalone（Node）：stdoutGuard + stdioFrames 帧装配    host-standalone/index.ts:73-156
```

- 开关 `MODE_HOST_INPROCESS_CHANNELS` 启动期一次性决定，为空 = 纯转发（inprocess.rs:44,105-116）。
- 白名单 25 个频道（inprocess.rs:72-101）；`mode-agent` 虽在白名单但按 S2 默认不注册。
- `host` 诊断频道**永不接管**（看门狗 `host.ping` 测的是 Node Host，搬进壳等于测壳自己）。
- 壳内 scoped 服务面已实现：控制帧归 Node，`LocalScopeRegistry` 登记，按 scope 惰性建
  `ChannelServer`，回帧加 `0xFF+scopeId` 前缀（rpc-host/scope_registry.rs、rpc-wire/scope）。

## 2. 裁决 1：帧层流控 = 有界队列背压 + V4 连接级流控；不引入 ACK 水位

**勘察事实**：

- 帧层（Node 路径与壳内路径共用）的真实背压是**入方向 256 帧有界队列**：满了读线程/
  `push_local_frame` 阻塞等待，不是丢帧（sidecar-host/process.rs:29-31,443-458）。
- `packages/rpc/src/persistent-protocol.ts` 的 `saturated`/`drained` 字节水位（1MiB/256KiB）
  **全仓零订阅者**；唯一实例化点 `rpc/src/remote.ts` 无生产调用方——是休眠抽象。
- 端到端在跑的流控是 **V4 连接级**：agent 端点 `saturated/drained/closed` → gateway 暂停/恢复
  → `setConnectionFlowStateV4` → 壳内 mode-agent 频道已接管（mode_agent.rs:2036-2047）。

**裁决**：W2 的「流控对拍」收口为——

1. 帧层不新增 ACK/水位机制；背压语义 = 256 帧有界队列（满则阻塞）+ OS 管道背压；
2. `PersistentProtocol` 保持休眠，不删除（远程 socket 层留作抽象），也不接线；
3. 队列饱和→恢复行为补 Rust 对拍测试（host_lifecycle.rs），替代「对拍」二字的实际内容；
4. V4 连接级流控已接管，属 W4 范畴，不在本文重复验收。

**退场注记**：`stdioFrames`/`stdoutGuard` 仅被 host-standalone 入口与测试/夹具引用
（≈200 行，Rust 侧等价物 `crates/rpc-wire`+`sidecar-host/frame` 已独立并有黄金对拍）。
它们随 **host-standalone 退出产品路径**（M4b-5/W6）一起退场，前置是 W3 的
`mode-task`/`bots`/`remote`/LAN WS/远程桌面协调器/provider 网络/automation/usage-stats
全部迁完，且 `host` 看门狗频道找到 Node 之外的承载。**W2 不删它们。**

## 3. 裁决 2：默认切换语义（unset 保持转发；`default` 特值 opt-in）

| `MODE_HOST_INPROCESS_CHANNELS` | 行为 |
| --- | --- |
| 未设置 | 纯转发（现状不变；默认翻转是真机验收后的**一行改动**，见 §5） |
| `none` | 纯转发（显式回滚，优先级最高） |
| `default` | 默认集 = 白名单 24 频道（**不含 `mode-agent`**，S2 决策）∪ 显式列出的白名单命中项 |
| 其它列表 | 现行为：按白名单过滤，未移植名忽略并留痕 |

- `default` 与 `none` 应单独使用；混用时 `none` 优先（回滚语义最强），其余项忽略并留痕。
- 默认集含 `agent-health`（只读健康面，已接线）；不含 `host`（永不接管）。
- 回滚路径不变：`none` / 列出部分频道 / 清空重启。

## 4. 裁决 3：scoped attachment 的对拍收口

壳内 scoped 路由/登记/回帧前缀已有单测（rpc-host/tests/router.rs 7 条、
host-channels/tests/pipeline.rs 3 条）。本文补齐的缺口：

- **本地 scoped 端到端**（pipeline.rs）：`scope-ready` 登记 → scoped 请求 → 本地
  `ChannelServer` 处理 → 回帧带 `0xFF+scopeId` 前缀 → detach 后同 scope 请求回落转发。
- **真机项**（需真实 remote attach，留在 §6 清单）：SSH/WSL 会话内发起 attach →
  scope-ready 由 Node 发放 → 壳内频道在 scope 上应答 → detach 清理；换代（Host 重启）
  后 base+scope 一并 dispose。

## 5. 默认翻转（已执行 2026-11-21；本节清单转为翻转后的回归验证）

> 状态更新：翻转已按用户指令执行（未设置 = §3 默认集）；验收清单从「翻转前置」
> 转为「翻转后的回归验证」，逐项结果回填 `.plan/accept-w6/report-<版本>.md`。
> 回滚 = `MODE_HOST_INPROCESS_CHANNELS=none`。

把「未设置 → 纯转发」改成「未设置 → §3 默认集」之前，以下必须全绿（记录进
`.plan/accept-<版本>-inprocess-default/`）：

| # | 项 | 状态 |
| --- | --- | --- |
| 1 | 19 频道本地真人路径验收（2026-10-31，`.plan/accept-0.0.109-inprocess/report.md`） | ✅ |
| 2 | `terminal` 真机验收（PTY 生命周期/输出泵/重挂接） | ⬜ |
| 3 | `mcp-sync` 真机验收（listWorkspaceMcpServerStatuses 词源） | ⬜ |
| 4 | `hooks` 真机验收（load/save/grant 转发） | ⬜ |
| 5 | `agent-health` 真机验收（只读健康快照） | ⬜ |
| 6 | 远程 scope 用例（§4 真机项，SSH 或 WSL 一条即可） | ⬜ |
| 7 | `MODE_HOST_INPROCESS_CHANNELS=default` 整机回归（启动/换代/退出无孤儿） | ⬜ |

全绿后的翻转改动：`resolve_channel_requests(None)` 返回默认集（一行），回滚 = `none`。

## 6. 验收场景（本文交付的部分）

1. 解析语义：未设置/`none`/`default`/`default+显式`/显式未移植名/mode-agent 显式——
   inprocess.rs 单测钉住。
2. 队列饱和→恢复：256 帧填满后 `push_local_frame` 阻塞；消费后完成且 FIFO 保序——
   host_lifecycle.rs 对拍测试。
3. 本地 scoped 端到端：登记 → scoped 请求本地应答 → 回帧带前缀——pipeline.rs 用例。
