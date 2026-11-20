# W2 翻面真机验收清单（spec w2-transport-cutover.md §5）

前置：真机构建含 inprocess 开关支持。单频道接管用
`MODE_HOST_INPROCESS_CHANNELS=<频道名>`；整机用 `=default`。
记录格式：每项一段「结果（PASS/FAIL）+ 现象 + 截图/日志引用」，写回本目录
`report-<版本>.md`。

| # | 项 | 操作 | 期望 | 状态 |
| --- | --- | --- | --- | --- |
| 1 | 19 频道本地真人路径 | （已完成 2026-10-31，`.plan/accept-0.0.109-inprocess/report.md`） | — | ✅ |
| 2 | `terminal` 真机验收 | 设 `MODE_HOST_INPROCESS_CHANNELS=terminal` 重启；在集成终端里：新建 profile → 跑 `ping localhost -n 3`（Windows）/`sleep 3`（unix）→ 分屏 → 关闭面板 → 重开面板 | 输出实时泵送；分屏各自独立 PTY；关闭/重开不残留进程（任务管理器核对）；重开后历史缓冲可用 | ⬜ |
| 3 | `mcp-sync` 真机验收 | 设 `=mcp-sync` 重启；打开 MCP 设置页 → 添加一个 stdio MCP server → 查看状态列表 | 状态词源与 Node Host 时代一致（enabled/工具数/连接态）；`listWorkspaceMcpServerStatuses` 的 mode 被拒时返回稳定码 `MODE_AGENT_MCP_STATUS_MODE_UNSUPPORTED` | ⬜ |
| 4 | `hooks` 真机验收 | 设 `=hooks` 重启；Hooks 设置页：载入（load）→ 保存（save）→ 对某 workspace 授信（grant） | 三操作均成功；授信后该 workspace 的 hook 不再重复询问 | ⬜ |
| 5 | `agent-health` 真机验收 | 设 `=agent-health` 重启；触发一次 agent 重启（设置里切运行时/手动重启 agent） | 只读健康快照在重启前后都返回（runtime/pid/startupId）；无写路径暴露 | ⬜ |
| 6 | 远程 scope 用例 | 常规 inprocess 开启下：SSH（或 WSL）会话内发起 remote attach → scope-ready → 在该 scope 里用壳内频道（如 setting/file）→ detach | attach 前 scoped 请求回落 Node；scope-ready 后本地应答（回帧带 0xFF 前缀，对端可解）；detach 后再回落；Host 换代后 base+scope 一并 dispose（无孤儿） | ⬜ |
| 7 | `MODE_HOST_INPROCESS_CHANNELS=default` 整机回归 | 设 `=default` 重启整机：冷启动 → 正常使用全部功能 → 退出 → 再启动 | 启动/换代/退出无孤儿进程（任务管理器无 node/agent 残留）；24 频道行为与单频道验收一致；`describe` 的 host 身份正常 | ⬜ |

全绿后的翻转：`resolve_channel_requests(None)` 返回 `default_owned_channels()`
（inprocess.rs 内一处；回滚 = `MODE_HOST_INPROCESS_CHANNELS=none`）。
