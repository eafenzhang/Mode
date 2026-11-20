# LT-15 生态面收口长任务（P1–P6）

状态：**执行中（2026-11-21，P1 完成）**。上位：
`docs/plans/2026-10-29-full-rust-rewrite.md` §10、
`docs/plans/2026-11-16-upstream-capability-long-tasks.md`。
本文件把剩余生态面收口收敛成**一个** program-level 长任务，按依赖顺序执行；
子项对应 `2026-11-21-remaining-long-tasks.md` 的 LT-8/9/10/12/14/11。

## 目标与边界

按「TUI 传输 → axum LAN/WS → 插件安装/卸载 → LAN WS/usage 网络 → C4 phase 投影 →
W6 收口」推进。每个阶段先补 spec/测试，再改代码；W6 只有在 P1–P5 的阻塞项清零后才执行
删除动作。

| 阶段 | 内容 | 出口条件 | 状态 |
| --- | --- | --- | --- |
| **P1** | TUI Agent 传输接线 | `agent-server` stdio spawn、sessions-index 订阅、live 增量、Ctrl-N 新建；单测 + 真子进程 e2e | ✅ 2026-11-21 |
| **P2** | axum LAN/WS | LAN 六端点全部桥接（state/pair/enable/pair-code/reset/clients 删除，11 测试；pair 含 Set-Cookie、enable 走监听视图）；剩 `/ws/host` RPC 帧桥接与凭据装配 | 🟡 HTTP 全收口 |
| **P3** | 插件安装/卸载进壳 | 本地目录/归档安装、卸载、manifest 校验；与 Agent/TS 行为对拍 | ⬜ |
| **P4** | LAN WS / usage 网络 | W3 剩余 LAN WebSocket/中继与 usage-stats 网络/凭据接线；注入 HTTP 测试 | ⬜ |
| **P5** | C4 phase 投影 | sessions-index 从会话快照派生 phase/预览/后台工作并 live 更新；19 PASS/1 PARTIAL 收尾 | ⬜ |
| **P6** | W6 收口 | `w6:readiness --fail-on-blockers` 通过；删 stdioFrames/stdoutGuard、TS 宿主编排与 host-sidecar 发运集 | ⬜ |

## P1 交付（2026-11-21）

- `crates/tui/src/agent_client.rs`：spawn `agent-server app-server --stdio`，按行 JSON-RPC
  收发；reader 线程自动应答反向请求；Drop kill/wait；env 覆盖（`MODE_TUI_AGENT_COMMAND` /
  `MODE_TUI_AGENT_ARGS_JSON`，`MODE_TUI_WORKSPACE`）。
- `crates/tui/src/agent_wire.rs`：`v4/conversation/frame` sessions-index 快照/增量解析 →
  现有 `agent_events` 归约；其他 topic/分片帧忽略。
- `crates/tui/src/main.rs`：启动即订阅 `sessions-index/<workspace>`；Ctrl-N 发
  `session/create`，live 增量刷新列表；Enter 仍为本地回显（发送面后续）。
- 测试：TUI 14 条单测；`crates/agent-server/tests/tui_client.rs` 1 条真子进程 e2e
  （spawn → 订阅 → 新建 → 收到 `sessions.upserted`）。

## P2 进展（2026-11-21）

- LAN 管理面 HTTP 全部收口：`lan-http` 抽出 `management_request` 传输无关入口（含令牌
  鉴权与错误码），`crates/server/src/lan.rs` 桥接六端点；`handle_pair` 透传 Set-Cookie；
  `set_enabled` 走监听视图（持久化待接 credential-store）。11 条 server 测试 +
  12 条 lan-http 测试全绿。
- 剩余：`/ws/host` RPC 帧桥接（与 P4 LAN WS 语义合并推进）、凭据装配从内存端口换
  credential-store。

## P3 首切

1. 安装：本地目录/归档 → manifest 校验 → 目标插件根原子落盘；先不接远端 marketplace。
2. 卸载：按 pluginId 定位、校验归属、删除并回报状态。
3. 与 `apps/mode-cli` 的 install/uninstall 行为对拍（输入/输出/错误码/文件树）。

## P4/P5 入口

- P4：`crates/lan-*` 的 WebSocket/中继缺口与 `crates/usage-stats` 网络/凭据接线。
- P5：把 TS `sessions-index-projection` 的 phase/预览/后台工作归约移植到
  `crates/agent-server/src/sessions_index.rs`，由会话事件驱动。

## P6 门禁

`pnpm w6:readiness --fail-on-blockers` 必须返回 READY；当前（2026-11-21）product 阻塞：
stdioFrames 6、stdoutGuard 6、host-sidecar 119、Node agent 回退 95。P1–P5 完成前不动删除。
