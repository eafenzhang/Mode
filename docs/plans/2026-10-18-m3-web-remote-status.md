# M3 生态域：Web 端与手机远控现状与决策（2026-10-18）

> 路线图 M3 的「Web 端（axum）」验收是 `web-remote-replayable` 链路 + 手机远控复用
> Host attachment 的既有架构（AGENTS.md：不为手机另起运行时）。
> 本文件按 LAN/IM bot/插件市场同一口径，先跑现有实现与测试，再给出 M3 的取舍。

## 结论先行

**Web 与手机远控由 Host sidecar 复用现有 `packages/server` + `packages/ui` 实现，
M3 不在 Rust 侧新增 `crates/server`。** 现有 156 条相关测试全绿；剩余是**两台设备的
人工端到端验收**（配对、attach、发 prompt、重连、远程桌面交互），以及发布形态下的
真实浏览器/手机验证。

## 归属证据

- `packages/desktop/src/host/lanAccessServer.ts` 在 **Host 进程**里
  `createHttpServer`（`@mode/server`），LAN/Web 入口与 Host 同生命周期；
  Tauri 的 Node sidecar 复用同一份 Host 代码（M3 生态计划 DR-M3-1）。
- `packages/desktop/src/host/index.ts` 的远程 backend（`@mode/server/remote`）负责
  SSH/WSL/Docker 与远程 workspace attach；手机远控复用 Host attachment，不另起 Agent
  （AGENTS.md「进程、协议与远程控制」）。
- `desktop-continuous`（桌面实时）与 `web-remote-replayable`（快照 + 队列重放）的边界
  已在 M2 收口：Rust 运行时只透传 `deliveryKind`，重放归 Host/relay（DR-T5-1 (a)），
  Host 侧重放由 `packages/desktop/tests/task-realtime-replay.test.mts` 覆盖。

## 测试证据（2026-10-18 实跑）

| 层 | 命令 | 结果 |
| --- | --- | --- |
| desktop 远程/LAN | `pnpm exec tsx --test packages/desktop/tests/remote-*.test.mjs packages/desktop/tests/remote-*.test.mts` | **40 pass / 0 fail** |
| UI（远程桌面） | `cd packages/ui && pnpm exec tsx --tsconfig tsconfig.json --test test/remoteDesktop*.test.ts` | **86 pass / 0 fail** |
| server | `cd packages/server && pnpm exec tsx --test test/*.test.ts` | **30 pass / 0 fail** |

覆盖范围：

- 会话状态机与重连：`sessionReducer`、`reconnectPolicy`、`sessionReconnectController`、
  `firstFrameWatchdog`、`nativeSession`；
- 媒体：`codecFallback`、`decoder`、`adaptiveBitrate`、`sessionStats`、`videoFit`、
  `media-relay`（desktop）；
- 输入与剪贴板：`input-path`（坐标映射/黑边 clamp）、`clipboardSync`；
- 设备与能力：`monitors`、`deviceAvailability`、`capability`（含对新对端 CUA 归属、
  旧对端回落）；
- 服务端：`lan-access-host`、`lan-cookie-compat`、`lan-workspace-recording`、
  `remote-desktop-session`、`remote-desktop-ticket`、`lan-server-info-*`；
- 远端 workspace：`remote-workspace-session-services`、`remote-prompt-attachments-order`、
  `remote-assets-github-release`、`remote-target-match`（含 SSH/WSL/Docker 判等口径）。

## DR-WEB-1：M3 不新增 `crates/server`（2026-10-18）

| 选项 | 含义 | 代价 |
| --- | --- | --- |
| **(a) Host sidecar 复用（采纳）** | LAN/Web HTTP 服务与手机 attach 继续跑在 Host 进程；M3 用现有测试验收 | 与 LAN/远程域一致；axum 重写没有收益 |
| (b) axum 重写 `crates/server` | 路由、鉴权、票据、远程桌面信令全部重写 | 与 Host sidecar 内既有实现重复；手机远控仍要复用 Host attachment，等于两套入口 |

(a) 的依据是 AGENTS.md 的「手机远控连接桌面已有 Host attachment，复用会话运行时；
不为手机另起 Agent、Local Host 或远程会话」。axum 重写只在 M4/M5 把 Host 移到 Rust
时重新评估。

## 仍未完成项

**必须由人完成（需要第二台设备 / 真实浏览器）**：

1. 两台机器配对（UDP 发现 + 6 位配对码），attach 后发一轮 prompt 并确认
   `desktop-continuous` 流式回复；
2. 手机端断线重连：确认 `web-remote-replayable` 的快照 + 队列重放不丢消息、不重复执行；
3. 远程桌面实机：多显示器切换、剪贴板双向、弱网降码率、首个关键帧超时后的降级；
4. 发布形态验证：Tauri 壳 spawn sidecar 后，真实浏览器/手机访问 LAN 入口
   （含 cookie 兼容窗口的口径：改名前版本不兼容，见 `lan-no-pre-rename-compat`）。

**可自动执行的后续**：

- 若要在 CI 里加真实 HTTP 端到端，需要先有发布形态的 sidecar 启动器；
  现阶段用 server 层测试 + Host 重放测试覆盖契约面即可。
- TUI（`ratatui`）域盘点：先确认旧 TUI 的场景清单，再判断是否属 M3。
