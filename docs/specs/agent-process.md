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

`crates/mode-protocol-client` 另有一条 `close_write` 测试：写端关闭后新请求失败、
读端仍能收到通知。
