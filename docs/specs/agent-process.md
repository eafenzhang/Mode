# Spec：Agent 子进程层（`crates/agent-process`）

状态：2026-10-29 实现（W4 最终态第一层：进程所有权迁壳的传输基础）。行为真源：

- `packages/services/src/mode-agent/modeAgentProcessManager.ts`（spawn 参数与生命周期，部分）
- `packages/services/src/mode-agent/modeStdioTransport.ts`（stdio 关闭与进程树清理的参考）
- `crates/sidecar-host/src/process.rs`（无控制台窗口、`taskkill /T /F` 的既有做法）

## 范围

- 按 `AgentProcessSpec`（命令、参数、cwd、env、请求超时）拉起子进程；
- stdin/stdout 接入 `mode-protocol-client`；stderr 逐行广播；
- 优雅关闭：关闭写端（stdin EOF）→ 最多等 `grace` → 超时整树强杀；
- 立即强杀：`kill()`；
- 平台细节：Windows `CREATE_NO_WINDOW`（GUI 壳不弹控制台），Unix `process_group(0)`
  让子进程成为进程组组长，强杀用 `kill(-pgid, SIGKILL)`；
- `kill_on_drop(true)`：调用方忘记 shutdown 时至少回收根进程。

**不在范围**（后续切片）：

- workspace → 进程注册表、空闲回收、健康判定、重启监听；
- provider/account 配置同步、`mode-agent` 频道迁移；
- TS 的 stderr drain 窗口、进程树快照（`captureProcessTreeSnapshot`）、stdio tap 开发代理；
- 进程组 id 探测与 Windows `taskkill` 失败后的二次兜底（当前失败只依赖根进程
  `start_kill`）。

## 启动规格解析

`resolve_agent_command` 按 TS `resolveDefaultModeAgentCommand` 的顺序选择命令：

1. `MODE_AGENT_SERVER_COMMAND`（trim 非空）：args 取 `MODE_AGENT_SERVER_ARGS_JSON`
   （必须是字符串数组，非法直接报错；空/缺省回落 `["app-server","--stdio"]`），
   cwd 取 `MODE_AGENT_SERVER_CWD`（trim 非空）否则 workspace；
2. monorepo dev dist：从 `upward_start` 向上找 `apps/mode-cli/packages/cli/dist/mode.cjs`，
   命令为随包 Node（`node_candidates` 第一个存在的文件）否则 `node`；
3. monorepo dev 源码：`apps/mode-cli/packages/cli/src/main.ts` + 向上找
   `node_modules/.bin/tsx`（Windows 优先 `tsx.cmd`）；
4. 打包资源：`<resources>/agent/glm/mode.cjs` + 随包 Node/`node`；
5. 都没有 → `AgentResolveError::NotConfigured`。

cwd 失效时回落到 workspace；再失效则交给 spawn 继承壳进程 cwd。
`presentation_surface == "desktop"` 时移除已有 `--surface`/`--surface=` 并追加
`--surface desktop`（孤立 `--surface` 不吞后续 option，与 TS 修复一致）。

**未移植**：Electron runtime 分支（Tauri 壳没有 Electron）、已部署 native binary 的候选链
（`findModeAgentRuntimeBinary` / `GLM_BINARY_PATH` / `~/.mode/server/agents/glm`）、
`supportsStorageStartup` 标记。native binary 兜底随所有权切换一起补。

## 运行时注册表（单进程不变量）

`AgentRuntimeRegistry` 保证**一个 workspace key 只允许一个 Agent 子进程**：

- key = `workspaceIdentity?.trim() || workspacePath`（与仓库 Workspace Identity 规则一致）；
- `get_or_spawn`：已存在且 transport 未关闭 → 复用并刷新 `last_used`；已关闭 → 先回收再重建；
  同一 key 的并发调用只真正 spawn 一次（每 key 一把异步锁），不同 key 互不阻塞；
- `invalidate(key)`：关闭并移除，下次调用重建；
- `reap_idle(older_than, grace)`：回收超过空闲阈值的实例，返回数量；
- `shutdown_all(grace)`：壳退出/换代时关闭全部；
- spawn 失败不缓存，下一次调用会重试。

**当前未做**：自动空闲看门狗（由宿主定时调用 `reap_idle`）、健康探测、provider/account
配置同步、进程换代事件。实时切换所有权前必须先接上这些调用点，并确保 Host 不再拉起
同一 workspace 的 Agent。

## 与 TS 的差异

| 差异 | 说明 |
| --- | --- |
| stderr 排空窗口 | TS 在关闭时等待 stderr 排空（1.8–3.25s）；这里关闭后直接 abort stderr 任务 |
| 进程树快照 | TS 在发 EOF 前保存树成员以便找回 detached 后代；这里依赖 `taskkill /T` 与进程组 |
| 强制终止顺序 | TS 先 `SIGTERM`/关闭再 `taskkill`；这里优雅等待后直接整树强杀 |
| 进程组探测 | 未实现 `ownedProcessGroupId` 的存活校验 |
| 日志 | 本 crate 不落日志；stderr 由调用方消费 |

## 关闭语义

1. `shutdown(grace)`：`ProtocolClient::close_write()` 排队帧 flush 后 drop writer →
   子进程收到 stdin EOF。
2. 在 `grace` 内退出：正常返回；客户端随后因 stdout EOF 关闭。
3. 超时：Windows `taskkill /PID <pid> /T /F`；Unix 对进程组发 `SIGKILL`；再等
   `2s` 回收根进程。
4. `kill()`：跳过优雅等待，直接执行第 3 步。

## 验收

`crates/agent-process/tests/process.rs`（5 条，真实子进程）：

- stdio 往返 + stderr `handled:<method>`；
- 优雅关闭：stdin EOF 后子进程退出，客户端关闭，后续请求 `Closed`；
- `stubborn` 子进程：grace 超时后被强杀，`pid_is_alive` 最终为 false；
- 启动即退出：请求以 `Closed`/`Transport` 失败；
- 不存在的命令：`AgentProcessError::Spawn`。

`crates/agent-process/tests/registry.rs`（7 条）：同 key 复用同一进程、identity 优先于路径、
并发 `get_or_spawn` 只 spawn 一次、transport 关闭后重建、`invalidate`/`reap_idle` 回收、
spawn 失败不缓存、`shutdown_all` 关闭全部。

`crates/agent-process` 启动规格单元测试（6 条）：env 覆盖（args/cwd）、非法 args JSON、
dev dist 向上查找与 Node 选择、打包资源分支、全部缺失时 `NotConfigured`、
`--surface desktop` 改写（含移除已有值、非 desktop 不修改）。

`crates/mode-protocol-client` 另有一条 `close_write` 测试：写端关闭后新请求失败、
读端仍能收到通知。
