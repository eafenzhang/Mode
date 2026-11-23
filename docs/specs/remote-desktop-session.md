# Spec：`remote-desktop` 会话状态机（16.4b 续段）

状态：**状态机 + 显示器校验 + 编码能力已落（2026-11-23）**——`remote_desktop_session`
（10 条契约测试）与 `remote_desktop_displays`（6 条）。真实媒体/信令链路与控制器侧未做。

## 1. 为什么先做状态机

`RemoteDesktopPort` 的五个会话方法（startSession/sendSignal/endSession/respondConsent/
setLocalDisplays）全部依赖媒体面。但**被控侧的状态机是纯逻辑**：准入判定、同意等待、
等待 WS 接线、两个 TTL、被控态通知。它不碰 socket、不 spawn 任务、不读环境变量。

把它先搬过来的价值：媒体面落地后只需要「接一个 socket 进来」，而不用同时发明
状态机——后者是这类功能最容易出错的部分（两个 TTL 缺一个就会把被控端永久卡在 busy）。

## 2. 状态图

```text
Idle ──request_session(允许)──► PendingConsent{client, deadline=+75s}
  ▲                                      │
  │                                      ├── allow ──► PendingSocket{client, deadline=+35s}
  │                                      │                    │
  │                                      │                    ├── attach_socket ──► Active{client}
  │                                      │                    └── TTL 到期 ──┐
  │                                      └── deny / 75s 到期 ────────────────┤
  └──────────────────────  stop / dispose  ─────────────────────────────────┘
```

**不变量**：任意时刻至多一个被控会话占用；`controlled` 仅在三个占用态之一时为 `true`，
且只在**真的变化**时通知订阅者。

## 3. 准入顺序（`evaluate`）

与 TS `evaluateRemoteDesktopRequest` + 单会话互斥逐条对齐：

| 顺序 | 条件 | 结果 |
| --- | --- | --- |
| 1 | disposed / kill-switch 命中 | `disabled` |
| 2 | 总开关关 | `disabled` |
| 3 | 已有会话（三个占用态之一） | `busy` |
| 4 | 其余 | 进入同意流 |

kill-switch 在第 1 位是刻意的：它**只能关不能开**，即使总开关是开的也压死整链。

## 4. 两个 TTL 与它们防的是什么

| 常量 | 值 | 缺了会怎样 |
| --- | --- | --- |
| `CONSENT_SAFETY_TIMEOUT_MS` | 75s | main 的同意窗失约（正常应由它 60s 超时先回）→ 被控端永久卡在 `pending_consent` |
| `PENDING_SOCKET_TTL_MS` | 35s | 「同意后从不连」→ 被控端永久卡在 `pending_socket`，且占用一直算 busy |

两者的后果都是**永久 busy**，用户只能重启应用。所以 status 机的 `tick()` 必须被端口
按周期调用。

## 5. 三条防御性校验

1. **同意回帖必须匹配 pending client**。错主的回帖若被接受，A 的同意窗会被 B 关掉
   → 返回「没有待处理的同意请求」（TS 同）。
2. **接线必须匹配 pending socket 的 client**。迟到的接线不能顶掉别人的会话。
3. **`controlled` 只在变化时通知**。`pending_consent → pending_socket → active` 三段
   都是「被控中」，中间两次转换不该各发一条事件（TS `lastControlled` 比较）。

## 6. 幂等事实与动态事件的区别

`onDidChangeControlledState` 是**幂等事实**：首个订阅者立即收到当前值（不进缓存、
不丢首帧）。这与 `remote` 频道的 `onDynamicConnectionEvent`（只发当前订阅者、不重放）
是两种语义，不能混。

## 7. 验收

`cargo test -p host-channels --test remote_desktop_session`（10 条）：

- kill-switch 与总开关都在准入前段拒绝，且 kill-switch 优先级更高；
- 第二个请求 `busy` 且不顶掉第一个；
- allow → `pending_socket`、deny → `idle`（可再次请求）；
- 错主同意回帖被拒；
- 75s 未回帖释放占用；
- 35s 未接线释放占用（「同意后从不连」不卡死）；
- 错主接线被拒，自己的接线进 `active`；
- `stop` 报告被终止的会话且空闲时返回 `None`；
- 被控态只在 `false→true`、`true→false` 各通知一次，退订后不再通知；
- `dispose` 终止会话并让后续请求一律 `disabled`。

## 8. 状态机验收

- 真实媒体/信令链路（`MediaStart`/媒体 WS/中继口）；
- 控制器侧（`startSession`/`sendSignal`/`endSession` 的远端协议）；
- `setLocalDisplays` 的显示器合法性校验（TS 有 7 条过滤规则）；
- 编码能力观测写入（`rd-encoder-capability.json` 的写侧，读侧已在频道里）。

## 9. 显示器校验与编码能力（已落）

两块都不依赖媒体面，所以先落（`host_channels::remote_desktop_displays`）。

### 9.1 `setLocalDisplays` 的校验

TS 的过滤器有 **7 条**，任一不满足即丢弃该条：
是对象、`displayId` 是非空字符串、`xPx`/`yPx`/`widthPx`/`heightPx` 四个都有限、
宽高都 > 0。

归一 6 条：`displayId` trim、`label` 非字符串给空串、坐标 `trunc`（**向零取整，
负数不是向下取整**）、宽高至少 1、`scaleFactor` 非有限或 ≤0 时取 1、
`primary` 只在严格 `true` 时为真。

**兜底**：非空列表里若一个 primary 都没有，第一条置为 primary——否则对端没有可投的
主显示器，会话画面会全黑（这个坑不报错，只是没画面）。

### 9.2 编码能力的三步

| 步骤 | 函数 | 语义 |
| --- | --- | --- |
| 从会话结果映射 | `encoder_capability_from_start_result` | 只有显式 `encoder:"software"` 才是 `software`，其余一律 `available` |
| 读 | `load_persisted_encoder_capability` | 缺失/损坏/未知枚举值 → `None`（无缓存） |
| 写 | `persist_encoder_capability` | 只允许白名单值落盘；目录不存在时自建 |

**为什么「其余一律 available」**：会话已经建流成功，能力面就只能是「可用」这一档。
字段缺失或形状异常时猜成 `unavailable` 会误报失败，让用户去查一个不存在的问题。

**为什么写侧也要校验白名单**：写入未知值后读侧会把它丢弃，形成「写得进去、读不出来」的
静默不一致。

## 10. 未做
