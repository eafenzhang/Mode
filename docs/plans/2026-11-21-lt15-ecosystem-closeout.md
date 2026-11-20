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
| **P2** | axum LAN/WS | LAN 六端点全部桥接（state/pair/enable/pair-code/reset/clients 删除，11 测试；pair 含 Set-Cookie、enable 走监听视图）；剩 `/ws/host` RPC 帧桥接与凭据装配 | ✅ HTTP 全收口 + `/ws/host` 帧桥 + LAN 凭据落盘 + 「最近打开目录」录制已接（25 测试）；remote-desktop/direct 面 W3 遗留另册 |
| **P3** | 插件安装/卸载进壳 | 本地目录/归档安装、卸载、manifest 校验；与 Agent/TS 行为对拍 | 🟡 记录层+卸载编排+安装编排+config 写回+网络源（git clone）已迁 `plugin-sync-service`（34 测试 + 3 对拍）；zip/npm/pip 源与 TS 对拍补充待做 |
| **P4** | LAN WS / usage 网络 | W3 剩余 LAN WebSocket/中继与 usage-stats 网络/凭据接线；注入 HTTP 测试 | 🟡 `usage-stats` 频道已接（getProviderBalanceSnapshot：凭据→fetcher→快照全链路，7 频道测试 + 壳内装配 + 降级语义）；LAN WS/中继待做 |
| **P5** | C4 phase 投影 | sessions-index 从会话快照派生 phase/预览/后台工作并 live 更新；19 PASS/1 PARTIAL 收尾 | ✅ phase/goalStatus/title/lastAssistantPreview 已接（turn 事件驱动 + store 读尾）；hasBackgroundWork/workflowActivity 诚实投影为空（依赖 W4 backgroundWorks 运行时状态迁移，见 P5 说明） |
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

## P3 进展（2026-11-21）

- **记录层已迁**：`crates/plugin-sync-service/src/installed_records.rs` 对齐 TS
  `normalizeInstalledPluginsState`（旧 map 形态、官方市场改名归一、scope 别名、
  非法条目过滤）与 `saveInstalledPlugins`（atomic-file 原子写，两空格缩进 + 尾随
  换行字节对齐）；5 条单测。
- **卸载编排已迁**：`plugin_management.rs` 对齐 TS `uninstallMarketplacePlugin` /
  `getPluginDataDir` / `getPluginCacheDir` / `legacyOfficialPluginId` /
  `sanitizePluginId`——先 splice + 原子保存记录，再按 `removeCache`/`keepData`
  删安装目录与用户数据目录（官方市场改名后旧 id 目录兜底）；7 条单测。
- **安装编排已迁**：`marketplace_install.rs`——manifest 读取、依赖闭包 BFS（跨市场
  阻断/循环检测）、directory/file/filesystem 源解析、真实版本读取（plugin.json 优先
  于条目）、cache 目录拷贝（旧 id 兜底）、记录 upsert、失败回滚；6 条单测。
- **config 写回已迁**：`config_write_back.rs`——`enable_plugins_in_config`（安装即
  默认启用，已有显式设置含 `false` 不覆盖）、`remove_plugin_from_config`（别名集清理
  `enabledPlugins`/`options`）、`add_suppressed_builtin`/`remove_suppressed_builtin`
  （suppression 去重幂等/清除遗留让状态自愈）；官方市场改名与 `mode-cua` 别名归一。
  编排接线：安装成功后 enable、卸载记录路径成功后 remove+remove_suppressed
  （`config_path=None` 跳过，对拍复用）；签名变更：`install_marketplace_plugin` 增
  `config_path` 参数并返回 `enabled_ids`，`uninstall_marketplace_plugin` 改返
  `Result<Option<_>, String>`（记录不存在幂等 `Ok(None)`，config 写回失败传 `Err`）。
  spec 已补 §4d；29 条测试（26 单测 + 3 对拍）。
- **网络源已迁**：`marketplace_install.rs`——github/git/url/git-subdir 四类走 git
  clone 子进程（`MODE_GIT_BINARY` 覆盖、90s 超时、网络型错误 3 次重试、`--depth 1`/
  `--branch`/`checkout` sha pin）；`ResolvedPluginSourceRoot { path, temp_dir }` +
  `cleanup()` 对应 TS `{path, cleanup}` 所有权（修掉了成功 clone 临时目录无人清理的
  生产泄漏）；`local` 源与裸字符串源接上 `pluginRoot` 基准目录（manifest 新增
  `plugin_root` 字段）；修正两处文案保真度（`directory` 缺路径必填校验、裸字符串
  错误带原始 source 文本、`file` kind 归入 invalid/unsupported——TS 仅 marketplace
  级 source 支持 `file`）；npm/pip/zip 显式不支持（与 TS UnsupportedPluginSourceError
  同文案）。差异记录 spec 已知差异 #7/#8。e2e 用本地 git 仓库当远端（clone/subdir/
  sha pin/失败清理），git 缺失自动跳过；37 条测试（34 单测 + 3 对拍）。
- 待做：zip/npm/pip 源、与 TS 行为对拍补充。

## P4/P5 进展

- P5 首切（2026-11-21）：`RuntimeWiring` 注入 `SessionsIndexHub`；`Shared::publish`
  在 TurnStarted/TurnCompleted/TurnFailed/SessionTitleUpdated 后调用
  `publish_sessions_index_upsert`（从 live 状态派生 Running/CompletedSuccess/Draft/Error
  phase + goalStatus + title，通过 `hub.upsert_summary` 广播）。
  预览/后台工作/workflowActivity 投影待做。
- P4 余额查询首切（2026-11-21）：`crates/provider-balance` 增 `ReqwestBalanceFetcher`
  （`ProviderBalanceFetcher` 的真实网络实现：GET + Bearer + 15s 超时；401/403 之外
  全部落 `request_failed`，与 TS readApiJson 口径一致；ring TLS provider 构造时装）
  + mock server 5 用例。官方套餐/额度链路随去智谱化已 dormant 短路
  （`assertOfficialServiceRemoved` 恒抛），无需接线。
- P5 收口说明（2026-11-21）：hasBackgroundWork/workflowActivity 的忠实投影依赖
  `backgroundWorks`/`workflowRuns` 的 live 运行时状态，而 Rust 侧这两个状态尚未迁移
  （`state.rs` 里 `background_jobs` 硬编码空，W4 C 面遗留）。sessions-index 现投影
  `has_background_work: false` + `workflow_activity: None` 是「无此状态」的诚实值
  （协议上 workflowActivity optional、布尔为默认值，不谎报）。硬造投影会违反
  「UI 不把局部状态当服务端事实」；等 W4 backgroundWorks 状态迁移后在
  `publish_sessions_index_upsert` 补派生（TS 口径：`backgroundWorks.some(status==="running")`
  + `deriveSessionWorkflowActivity`）。
- P4 服务面接线（2026-11-21）：`crates/host-channels` 新增 `usage_stats` 频道
  （`getProviderBalanceSnapshot`：`{providerId?}` → 快照 JSON；缺省/undefined 与 TS
  `?? ""` 同语义进 unsupported，不是参数错误）+ 壳内装配（inprocess.rs：MODE env
  优先、data_root/v2 回退构造 PersonalConfigRepository，1s 轮询；仓库/客户端构造失败
  降级 default unsupported 并留 shell 日志，不阻塞注册）+ 频道名漂移守卫。
  spec 新建 docs/specs/usage-stats-service.md；官方套餐链路 dormant 不接线。
- P4 剩余：`/ws/host` RPC 帧桥接（= P2 剩余，合并推进）。实施路径：crates/server
  构造 ChannelServer 并复用 host-channels 全频道（inprocess.rs 的装配清单抽出为
  可复用模块），axum WS upgrade + lan-pairing 长期令牌 → 客户端记录解析 → 帧双向
  转发（rpc-wire 帧，与桌面 stdio 同格式）；`/ws/remote-desktop`/`/ws/remote-desktop-media`
  依赖远程桌面流面（W3 遗留），`/ws/v1/direct` 依赖 v3 直连协议。
- P4 凭据装配首切（2026-11-21）：`provider-balance` 增
  `provider_balance_target_from_personal_layer`（只有 ApiKey access 参与，
  ZhipuAccount/未配置 → None）+ `PersonalLayerTargetResolver`（读层闭包 →
  resolver trait，同步读取 + ready future）。effectiveConfig 的 builtin 模板合并
  待 ProviderSettingsView 移植；服务面端点接线（server/agent-server 的 usage 面
  组装 resolver + fetcher）与 `crates/lan-*` WebSocket/中继待做。

## P6 门禁

`pnpm w6:readiness --fail-on-blockers` 必须返回 READY；当前（2026-11-21）product 阻塞：
stdioFrames 6、stdoutGuard 6、host-sidecar 119、Node agent 回退 95。P1–P5 完成前不动删除。
