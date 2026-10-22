# IM 机器人在远程工作区的会话绑定（target 匹配、实例唯一与 identity 口径）

## 背景（缺陷）

`bindBotToTask` 的绑定链路要为 bot 上下文建立流观看：

```text
UI 绑定菜单 → botsService.bindBotToTask
  → focusBotOnWorkspace（已写入上下文）
  → ensureContextStreamWatch → watchTaskStream
  → resolveModeTaskServiceForContext（带 workspaceIdentity）
  → botRemoteWorkspaceBridge.getModeTaskService
  → parentPort → main createBotRemoteWorkspaceRuntimePort
  → routesBySessionId 按 {webContentsId, workspacePath, workspaceIdentity, target} 找 route
```

main 侧按 target 判等的唯一口径是 `isSameRemoteTarget`
（迁移前的 `packages/desktop/src/main/remoteTargetMatch.ts`；Tauri Host 线已收口到
`packages/shared/src/remoteTarget.ts`，见下面的「Tauri Host 的直连接线」），三个消费方共用：

1. `hasRemoteWorkspaceSessionForTarget`——Bot 桥的连接状态查询（`isConnected`）；
2. `createBotRemoteWorkspaceRuntimePort`——绑定/心跳/入站消息建立流观看时的 runtime attachment；
3. `reconnectBotRemoteWorkspaceSession`——断线后复用已有 session 还是新建。

该函数曾**缺 `lan` 分支**（并残留 `RemoteTarget` 中不存在的 `server` 分支）：
两个完全相同的 lan target 从 switch 掉出返回 `undefined` → 判否。后果对 LAN
远程工作区是确定性的：连接状态永远「未连接」、runtime port 永远
「未找到可供 Bot attachment 的远程 logical session」、`/重连` 永远新建 session；
`bindBotToTask` 必抛错，而绑定菜单 `handleBind` 只有 `finally` 没有 `catch`，
用户点「绑定」**没有任何提示与效果**——即「远程工作区无法绑定 IM 机器人」。

## Tauri Host 的直连接线（2026-10-22）

迁移到 Tauri Host sidecar 后，`BotsService` 与 `windowRemoteConnectionRegistry` 在**同一个
Host 进程**里（`packages/desktop/src/host-standalone/serviceAccessor.ts` 装配），
`parentPort → main` 这一跳不再存在。Bot 桥改为接受 Host 注入的**直连传输**：

```text
botsService → botRemoteWorkspaceBridge（transport）
  → serviceAccessor 注入的实现（延迟绑定，remote 服务创建后赋值）
  → remoteWorkspaceService.queryBotConnectionStatus / reconnectBotWorkspace / createBotRuntimePort
  → windowRemoteConnectionRegistry（唯一所有者）
```

| 方法 | 语义 |
| --- | --- |
| `queryConnectionStatus` | 按 `isSameRemoteTarget`（四种 kind 全覆盖）+ `workspaceIdentity` 扫 online session。查询**异常**返回 `null`（桥回退到本地标记），不把「查不到」伪装成「未连接」 |
| `reconnectBotWorkspace` | 同 target + identity 已有 online session 就**复用**（不新建 logical session、不广播）；否则走 `registry.connect`（底层连接按 target 复用），成功后广播 `botReconnected` 动态事件，`target` 先脱敏 |
| `createBotRuntimePort` | 为 Bot 建**进程内** scoped attachment：远端服务集合经 `MessageChannel` + `ChannelServer` 暴露成 RPC 端点，端口直接交给桥；**不经过**页面桥/帧解复用，也没有 `scope-ready`。端口由桥按 workspace 缓存，桥 `dispose` 或 Host 退出时关闭 |

renderer 侧由 `desktopRemotePlatformBridge` 订阅 Host 的 `botReconnected` 并转成
`onBotRemoteWorkspaceReconnected`；`window.mode` 只在 Host 服务缺席时作为 fallback
（Tauri 注入层该成员仍是留痕空操作，不伪造事件）。

## 产品规则

- **target 判等必须覆盖 `RemoteTarget` 全部 kind**（ssh / wsl / docker / lan），
  新增 kind 时同 PR 补分支；比较字段只取定位对端所需的 authority：

  | kind     | 比较字段                                                |
  | -------- | ------------------------------------------------------- |
  | `ssh`    | host（小写）、port（缺省 22）、username、privateKeyPath |
  | `wsl`    | distro（缺省 default）、user                            |
  | `docker` | container                                               |
  | `lan`    | host（小写）、port                                      |

  secret（token、密码、私钥口令）与展示性字段（serverName、sshConfigAlias）
  **不参与比较**——settings 快照与连接期 live target 的 secret 形态不同，
  比较它们必然误判为「不同 target」。

- **绑定动作的失败必须可见**：绑定/解绑菜单项的 RPC 异常要落日志并 toast，
  不允许静默吞掉（服务端 `{ok:false, reason}` 走既有 reason 文案，不受影响）。

- **进程内只有一个 BotsService 实例（唯一所有者）**：bot 配置、绑定表与状态
  都是本机全局单文件（`getAppConfigDir()`），而绑定码、流订阅、心跳、workspace
  refs 缓存都活在实例内存里——第二个实例必然造成分裂（一个实例发的绑定码另一个
  实例验不过；两实例同时订阅同一任务会把助手回复重复镜像到 IM）。
  远程 workspace scope（`remoteWorkspaceServiceCollection.ts`）必须**复用 Local
  Host 的 IBotsService**，不得再 `createBotsService` 自建；连接 dispose 会调用
  集合内服务的 `disposeAll*`，复用时必须屏蔽这两个所有权方法（远程连接断开不能
  关停本机 bot 运行时）。随之而来的正确性收益：远程 scope 实例原本没有
  `remoteWorkspaceService` 桥，任何带 `workspaceIdentity` 的操作必抛
  「runtime 不可用」；复用后走 Local Host 桥，与桌面 UI、IM 入站同源。

- **identity 数据口径：读出即补齐，存量 key 一次性升级**：
  - settings 读出（`appSettingsSchema` / patch）时，`lastWorkspaceSession` 的
    remote 条目缺 `workspaceIdentity` 必须用 shared 的唯一构造器
    `buildRemoteWorkspaceIdentity(path, target)` 补齐——UI 恢复、bot 绑定 key、
    Bot 远端桥查连接全都按 identity 口径工作，path-only 旧条目会让同一条远端
    工作区在各处算出不同 key。
  - BotsService 首次读取绑定数据时（`ensureBotWorkspaceKeysUpgraded`，一次/进程，
    读 `getConfig`、绑定读写入口触发）把存量 path-only key 升级为 identity key：
    绑定表 key 用保守规则 `resolveLegacyBindingWorkspaceKey`（path 若本身就是
    某个本地工作区的 key 则保持不动——本地工作区没有 identity，无法区分
    「旧远端绑定」与「本地绑定」，宁可不升级也不误绑）；
    `allowedWorkspaces` 沿用既有 `normalizeConfiguredAllowedWorkspaces` 归一。
    不升级会让会话绑定资格（按 identity 查 `getWorkspaceBoundBots` /
    `isBotEligibleForSessionBinding`）永远看不到存量绑定，表现为
    「该机器人不属于当前工作区，无法绑定」。

## 验收场景

1. 回归测试 `packages/shared/test/remote-target-match.test.ts`（CI
   `pnpm --filter @mode/shared test` 覆盖；判等函数已随迁移收口到
   `packages/shared/src/remoteTarget.ts`）：同 host+port 的 lan target 判等为 `true`，
   port/host 不同判 `false`，kind 不同判 `false`，ssh/wsl/docker 口径按上表，
   secret 与展示字段不参与。
2. 回归测试 `packages/services/test/bot-remote-workspace-identity.test.ts`
   （CI `pnpm --filter @mode/services test` 覆盖）：
   - settings 读出缺 identity 的 remote 条目被补齐为统一构造器的派生值；
   - 预置 path-only 绑定表 + path-only `allowedWorkspaces` 后，首次绑定读取把
     远端工作区升级到 identity key（`getWorkspaceBotBinding(identity)` 与
     `getConfig().allowedWorkspaces` 都能看到），本地工作区绑定保持 path key
     （修复前：identity 查询为空、授权仍是 path，必红）；
   - `remoteWorkspaceServiceCollection.ts` 不再出现 `createBotsService`，改为
     `getOptional(IBotsService)` 复用并屏蔽 `disposeAll*`（修复前：仍自建实例，必红）。
3. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 全绿。
4. 实机（LAN 对端已配对且工作区已打开）：右键会话 → IM 机器人 → 绑定，
   成功 toast 且绿点亮起；断开后 Bot 侧消息收到「远端未连接」而非静默。
5. Tauri Host 直连（2026-10-22）：
   - `packages/desktop/tests/remote-workspace-host-service.test.mts`「Bot 远端桥」+
     `packages/services/test/bot-remote-workspace-transport.test.ts`：状态查询按
     target + identity 命中/拒绝；重连复用时不广播、新建时广播 `botReconnected`；
     runtime port 经进程内 `MessageChannel` 真能 RPC，桥 `dispose` 关闭端口；
   - `packages/desktop/tests/desktop-platform-lan-delegation.test.mts`：Tauri + Host
     时 `onBotRemoteWorkspaceReconnected` 订阅 Host 动态事件，disposer 生效。
