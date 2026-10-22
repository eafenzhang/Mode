# 远端 workspace 断连后的有限自动重连

> 2026-10-22 产品决策 B：已建立的远端连接异常断开后自动重试，**最多 3 次，退避 1s / 3s / 9s**；
> 触底后停在断连态并提示，用户可手动重连。启动阶段仍然**不**自动连接。

## 背景

远端 workspace（LAN / SSH / WSL / Docker）的连接由 Host 的 `windowRemoteConnectionRegistry`
持有。会话关闭后，renderer 过去只把 tab 降级为断连态（清 `remoteSessionId`、把运行中任务收口为
failed），重连完全依赖用户点击。一次网络抖动或对端进程瞬时退出，就会让整个 workspace 停在
断连态，需要人工恢复。

启动时自动重连曾被移除：用户只是打开应用查看任务列表，不应触发远端连接与 runtime 上传。
本次保留该约束——自动重连**只由断连事件触发**，不由启动触发。

## 状态所有权

| 事实 | 所有者 | 说明 |
| --- | --- | --- |
| 断连事实 | Host `onDynamicConnectionEvent({kind:"closed"})` → renderer `onRemoteSessionClosed` | 后端 `error` 随事件透传，用于判断可重试性 |
| 重连调度 | renderer `useRemoteWorkspaceHistory`（每 workspace 一条调度链） | 纯逻辑在 `packages/ui/src/lib/remoteWorkspaceAutoReconnect.ts` |
| 连接与 session | Host `windowRemoteConnectionRegistry` | 自动重连只是**发起方**，不另建 session/registry |
| tab 连接态 | `tabStore.remoteSessionId` | 断连时清空；重连成功后由既有 `reconnectRemoteWorkspaceHistoryEntry` 回填 |

自动重连与手动重连、Bot 重连走**同一条** `reconnectRemoteWorkspaceHistoryEntry` 路径；
不新增第二条建连实现，也不写远端快照。

## 规则

1. **触发**：仅 `onRemoteSessionClosed` 且该 workspace 仍有 tab。
2. **可重试分类**（`classifyRemoteSessionClose`）：

   | 关闭形态 | 重试 |
   | --- | --- |
   | `error` 命中鉴权/版本/协议类（auth / unauthorized / forbidden / permission denied / credential / token / version / protocol / unsupported / mismatch） | 否 |
   | `exitCode === 0` 且无 signal、无 error（对端主动干净退出） | 否 |
   | `signal` 非空（被信号带走） | 是 |
   | `exitCode` 非零 | 是 |
   | LAN WS 断开（exitCode/signal 均 null） | 是 |
   | 其它未知 `error` | 是 |

   错误文本匹配是保守的：只在明确命中不可重试词时拒绝，其余一律重试——宁可多试两次，
   也不要把一次可恢复的断连直接判死。
3. **次数与退避**：3 次，依次等待 1s、3s、9s（`REMOTE_AUTO_RECONNECT_DELAYS_MS`）。
   每次尝试复用 `reconnectRemoteWorkspaceByKey`（`activateWorkspaceAfterReconnect: false`、
   `showErrorToast: false`、`throwOnFailure: true`）。
4. **成功**：停止该 workspace 的重试链；tab 回填 `remoteSessionId`，保持既有激活语义
   （自动重连不主动切换用户当前 tab）。
5. **触底**：停在断连态（既有失败落盘已记录 `lastConnectionError`），toast 一次
   `remoteWorkspace.autoReconnect.failed`（无错误详情时用 `…failedGeneric`），并 log warn。
6. **取消**：
   - 用户手动重连（侧栏重连 / 从远程历史打开）→ 取消该 workspace 的待执行尝试；
   - Bot 重连成功事件 → 取消该 workspace 的待执行尝试，并按 `requestId` 取消 UI 侧正在建连的
     connect（避免同一 workspace 出现两个 session）；
   - 用户移除 tab → 取消；
   - 组件卸载 → `cancelAll()`。
7. **并发保护**：每次尝试前检查 `inflightReconnectWorkspaceKeys` 与
   `pendingReconnectRequestIds`；已有重连在跑时让位并停止本组重试，不叠加第二条链。
8. **启动**：不自动重连。启动恢复仍只还原断连态 tab。

## 事件顺序

```text
Host: connection closed
  → renderer onRemoteSessionClosed
      → tab 降级（清 remoteSessionId）+ running 任务收口 failed + 失败落盘
      → classify(exitCode, signal, error)
          ├─ 不可重试 → 记录 detail，结束
          └─ 可重试 → scheduler.schedule(workspaceKey)   [每条链 1s → 3s → 9s]
                attempt N: reconnectRemoteWorkspaceByKey(throwOnFailure)
                    ├─ 成功 → 停止
                    ├─ 失败且 N<3 → 下一条退避
                    └─ 失败且 N=3 → toast + 停止（断连态保留）
  取消信号：手动重连 / Bot 重连 / tab 关闭 / 卸载
```

## 验收

1. `packages/ui/test/remoteWorkspaceAutoReconnect.test.ts`：
   - 分类表（干净退出/鉴权/版本不重试；WS 断开、非零退出、信号、未知错误重试）；
   - 退避表固定 1s/3s/9s、共 3 次；
   - 三次失败触底、带出最后一次错误；
   - 中途成功即停；cancel（未执行与执行中）、重复 schedule 替换旧链。
2. `packages/desktop/tests/desktop-platform-lan-delegation.test.mts`：Host 关闭事件的
   `error` 必须透传到 `RemoteSessionClosedEvent`（否则分类永远拿不到鉴权/版本信息）。
3. `packages/ui/test/remoteWorkspaceSessionRuntime.test.ts`：断连降级的任务收口行为。
4. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 全绿。
5. 真机（LR-5）：拔网/杀对端进程后观察自动重试 1s/3s/9s 与触底提示；Bot 重连与手动重连
   不应与自动链并发建连。
