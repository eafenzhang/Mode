# 远程控制 v3 Web 完整工作区页面

状态：2026-11-11 已实现（`connectRemote({kind:"v3"})`、attachment、浏览器 v3 服务与
向导接入）；真机双端验收待执行。母规范：
[remote-control-v3.md](./remote-control-v3.md)；Web 客户端现状见
[remote-control-v3-web.md](./remote-control-v3-web.md)。

## 1. 问题

`/v3` 目前只有设备目录、加密 RPC、工作区隧道连通测试。用户能把 v3 Host 的
`/ws/host` 挂成浏览器 `ISocket`（`openWebWorkspace`），但没有页面把这条连接变成
可用的工作区：没有 tab、文件树、终端、Git，也没有「打开哪个工作区」的选择。

主 Web 应用（`packages/web`）已经有一套远程工作区主控链路
（[web-remote-workspace.md](./web-remote-workspace.md)）：

- `attachRemoteWorkspaceSession()` 接管服务端远程连接，注册
  `registerRemoteWorkspaceSession()`；
- `connectRemote(target)` 按 `RemoteTarget.kind` 分派（lan / ssh / docker）；
- 连接向导、目录浏览、远程 tab 都消费同一个 `remoteWorkspaceSessionStore`。

缺的是 `kind: "v3"` 这一条分派与设备来源。

## 2. 目标

1. 主 Web 应用里登录 v3 账号（复用 `/v3` 已存的 IndexedDB 凭据），列出同账号 Host；
2. 连接向导选择 v3 Host，`connectRemote({ kind: "v3", deviceId })` 经 Noise 建
   `/ws/host`，注册为远程 session；
3. 之后的 tab / 文件 / 终端 / Git / 设置全部走远端 services，与 LAN 路径一致；
4. 关闭 tab 或断线时释放链路，不残留 Control/业务连接。

非目标：把 `/v3` 独立页做成第二个 app shell；远程桌面页面（已单独落地）。

## 3. 现有证据

| 位置 | 现状 |
| --- | --- |
| `packages/web/src/main.tsx` `connectRemote()` | 只处理 `lan` / `ssh` / `docker`，`v3` 落入 SSH/Docker 分支被拒 |
| `packages/web/src/main.tsx` `attachRemoteWorkspaceSession()` | 只接服务端 `/ws/remote/:id`，v3 没有服务端 session id |
| `packages/client/src/webWorkspace.ts` | 已能把 v3 `/ws/host` 包装成 `ISocket` + `ChannelClient` |
| `packages/ui/src/store/remoteWorkspaceSessionStore.ts` | renderer 侧唯一 remote session 注册表，可直接复用 |
| `packages/desktop/src/host/remoteV3/rpc.ts` | Host 只有 `remote.workspace.list`，没有按路径打开工作区的 RPC |
| `packages/shared/src/remoteTarget.ts` | `V3ConnectOptions` 只有 `deviceId`/`deviceName`，没有 workspacePath |

## 4. 方案

### 4.1 边界与所有者

```text
主 Web（浏览器）
  ├─ v3 账号会话（WebAccountClient + credentialStore）   —— 账号/设备目录的唯一读取者
  ├─ WebControlLink（Noise + Control/Relay）              —— 连接的唯一所有者
  ├─ openWebWorkspace（/ws/host → ISocket/ChannelClient） —— 工作区 RPC 的唯一入口
  └─ remoteWorkspaceSessionStore                          —— 远程 session 的唯一注册表

Host（被控）
  ├─ remoteV3 业务 allowlist（RPC）
  └─ 本机 LAN 服务（/ws/host、文件、终端、Git）
```

- 不新增第二套远程 session 存储：v3 与 LAN/SSH/Docker 都注册进
  `remoteWorkspaceSessionStore`。
- 不在 v3 页里内嵌 app shell：`/v3` 继续负责登录/设备目录/远程桌面；工作区入口进主 Web。

### 4.2 接口

v3 连接复用现有签名，不扩展协议：

```ts
// 已有
connectRemote(options: RemoteTarget): Promise<{ success: true; sessionId } | { success: false; error: string }>

// 本次在 web 平台实现
if (options.kind === "v3") {
  const profile = await loadV3Profile();            // 无凭据 -> 返回 error: "v3_login_required"
  const link = new WebControlLink({ ...profile });
  await link.start();
  await link.connect(options.deviceId);
  const workspace = await openWebWorkspace({ link, hostDeviceId: options.deviceId });
  const sessionId = crypto.randomUUID();
  registerRemoteWorkspaceSession({
    sessionId,
    target: options,
    services: workspace.services,
    dispose: () => { workspace.dispose(); link.close(); },
  });
  return { success: true, sessionId };
}
```

连接向导的设备来源沿用既有 `services.remoteV3Service.listHosts()`（`useV3ConnectionForm`），
Web 侧实现浏览器 Client 版服务：

```ts
createWebRemoteV3Service({ store, loadProfile })
  .listHosts()      // WebAccountClient.listHosts → role=host 投影
  .listDevices()    // 全部设备
  .getState()       // unconfigured / online / error + pendingLogin
  .beginGithubLogin/pollGithubLogin/cancelGithubLogin  // 浏览器内 Device Flow + 轮询
  .configure()      // Web 会话 token 直接注册设备
  .disconnectAccount()
  .request/httpRequest/openTunnel/sendTunnel/closeTunnel  // 按 Host 复用同一条 Control/Noise 连接
```

Host 管理类操作（`resetPeerPin`）与浏览器不适用的 `startRemoteDesktop`
（Web 远程桌面走 `/v3` 入口）明确 reject，不伪造成功。

### 4.3 事件顺序

```text
向导选择 v3 Host
  → connectRemote({kind:"v3"})
  → link.start / link.connect(host)
  → openWebWorkspace(/ws/host, Initialize)
  → registerRemoteWorkspaceSession
  → 向导关闭，shell 按 sessionId 打开远程 tab

关闭 tab / 断线
  → session.dispose()
  → workspace.dispose()（关隧道）
  → link.close()（Control/Relay 收口）
  → unregisterRemoteWorkspaceSession
```

## 5. 失败语义

| 场景 | 行为 |
| --- | --- |
| 未登录 v3 | `connectRemote` 返回 `v3_login_required`，向导提示先登录 |
| Host 离线 | `link.connect` 超时/拒绝 → `success:false`，不注册 session |
| Host LAN 服务未启用 | `openWebWorkspace` 返回 `HOST_LOCAL_API_UNAVAILABLE`，提示在被控端启用本机服务 |
| 隧道中途断开 | `ChannelClient.notifyTransportClosed` 拒绝在途 RPC，session 标记断开；重连按 v3 隧道策略 |
| 凭据失效 | Control 401/403 → 不重试，session 收口并提示重新登录 |

## 6. 验收

1. 主 Web 登录 v3 账号后，向导出现同账号在线 Host；选择后打开远程 tab。
2. 文件树、终端、Git、设置在远程 tab 内可用，数据来自被控 Host。
3. 关闭 tab 后 Host 侧无残留连接（Control 连接数回落，`/ws/host` 隧道关闭）。
4. 未登录/离线/本机服务未启用分别命中 §5 的错误语义。
5. 单测：`connectRemote({kind:"v3"})` 注册/释放 session；向导 v3 列表为空态与离线禁用。

## 7. 工作量

| 工作项 | 预估 |
| --- | --- |
| web `connectRemote` v3 分支 + 凭据读取 | 已完成 |
| 向导 v3 设备来源（浏览器 `remoteV3Service`） | 已完成 |
| 会话释放/断线与现有 store 对齐 | 已完成 |
| 测试与回归 | 已完成（7 项：attachment 2 + 浏览器服务 5） |
| **合计** | **实际约 2 轮** |

剩余：真机双端验收（v3 Host 在线 → 向导选设备 → 打开远程 tab → 关 tab 释放）。

## 8. 已定决策

- 账号会话：主 Web 复用 `/v3` 写入的 `mode.remote.v3.web-server-url` + IndexedDB
  profile 静默恢复；`allowRemoteWorkspace` 在「LAN 主控或存在 v3 账号」时放开。
  主 Web 账号弹窗的 v3 登录也由同一服务实现（Device Flow 在浏览器内轮询）。
- 远程工作区选择：沿用 LAN 路径的目录浏览（连上后由 shell 选目录），
  `V3ConnectOptions` 不增加 `workspacePath`。
- 请求/隧道所有权：同一 Host 复用一条 `WebControlLink`；`disconnectAccount` 统一收口，
  session dispose 只关自己的 `/ws/host` 隧道。
