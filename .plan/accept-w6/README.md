# W6 收口验收包（Node Host 退役专项）

状态更新（2026-11-21）：
- **W2+S2 翻转已执行**（用户指令；`resolve_channel_requests(None)` = 默认集 +
  mode-agent）。两份清单转为**翻转后的回归验证清单**——逐项执行并回填
  `report-<版本>.md`；发现回归时回滚 = `MODE_HOST_INPROCESS_CHANNELS=none`。
- **物理删除仍未开窗**：renderer 仍消费 Node Host 独占面（mode-task 任务系统、
  window-controller 窗口控制、remote-desktop/remote-v3/lan-access 等，不在
  inprocess 白名单）——这些面迁入 Rust 前删除会断任务/窗口/远程功能。删除窗口的
  前置从「两份清单全绿」扩大为「**Node Host 独占频道全部迁入 Rust**」。

## 1. 门禁链全景

```
W2 翻面（业务频道默认集）──┐
                          ├─→ 全绿 ──→ Node Host 退役（W6 物理删除）
S2 mode-agent 翻面 ────────┘
```

| 步骤 | 内容 | 门禁 | 状态 |
| --- | --- | --- | --- |
| W2 翻面 | `MODE_HOST_INPROCESS_CHANNELS` 默认集接管 24 业务频道 | `w2-flip-checklist.md` 六项全绿 | **翻转已执行**（回归验证待真机：1/7 已跑） |
| S2 翻面 | `mode-agent` 频道注册进默认集 | `s2-mode-agent-checklist.md` 20 任务全过 | **翻转已执行**（回归验证待真机；适配器 88/100 + 15 事件已接） |
| W6 删除 | 删 stdioFrames/stdoutGuard、host-standalone、host-sidecar 发运集、mode.cjs 回退 | **Node Host 独占频道全部迁入 Rust**（mode-task/window-controller/remote-* 面） | ⬜ |

## 2. 为什么现在不能删（2026-11-21 评估）

`node scripts/w6-readiness.mjs` 的四组 blocker **全部是活代码**（逐文件核对）：

- `packages/desktop/src/host-standalone/index.ts` 是 Node Host sidecar 的运行入口；
  `stdioFrames`/`stdoutGuard` 是它与桌面壳的现役 stdio 链路。
- `apps/desktop-tauri/src-tauri/src/host.rs`/`inprocess.rs` 的 sidecar 编排是壳的
  现役降级路径（翻面开关未翻转时 Node Host 承载一切）。
- `mode.cjs`/`bundled-agents` 是 agent 的回退与开发链（生产 agent 已默认 Rust
  native binary——C4 翻转已上线，回退仍在）。

物理删除会断掉桌面壳的降级路径与开发链；删除只能在翻面验收全绿后执行。

## 3. 验收执行方式

1. 真机安装/启动当前构建（含 inprocess 开关支持）。
2. 按 `w2-flip-checklist.md` 逐项执行并记录（每项含操作步骤/期望/失败回滚）。
3. 按 `s2-mode-agent-checklist.md` 执行 20 任务并记录。
4. 全部记录写回本目录（`report-<版本>.md`，格式沿用
   `.plan/accept-0.0.109-inprocess/report.md` 的先例）。
5. 全绿 → 提交翻转 PR（W2 一行 + S2 白名单一行）→ 真机回归一遍
   （清单 #7 整机回归）→ 开 W6 删除窗口。

## 4. 翻转改动（已执行 2026-11-21）

- **W2**：`resolve_channel_requests(None)` 返回 `default_owned_channels()`。
- **S2**：`default_owned_channels()` 加入 `MODE_AGENT_CHANNEL`；两个钉住旧决策的
  测试（`unset_switch_stays_forward_only`/`default_expands_without_mode_agent_or_host`）
  已按新语义改写（`unset_switch_takes_the_default_set_with_mode_agent`/
  `default_expands_with_mode_agent_but_never_host`）。
- 回滚：`MODE_HOST_INPROCESS_CHANNELS=none` 永远是纯转发（单测钉住）。

## 5. 已知保真缺口（不阻塞翻面，随面迁移收口）

- `sendPrompt` 的 `browserAmbientContext` 主动采集：协议透传已接
  （`SendPromptParams.browser_ambient_context` → wire），「缺省时主动采集」依赖
  `browserControlExecutor`（浏览器控制面仍在 Node 侧未迁移）——随浏览器面迁移收口。
- `createSession` 的 provider 预同步：依赖 Node Host 的 provider 编排——随
  provider 面迁移收口。
- automation 10 方法：决策 4 `not-needed`（Renderer 分调两个 owner）；
  provider/account 2 网络方法：决策 2 显式 `unsupported_method`。
