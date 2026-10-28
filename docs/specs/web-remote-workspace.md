# Spec：Web / fpk 作为远程工作区主控（P2）

状态：2026-10-28 已实现 LAN 路径（真机双端验收待执行）。关联：
[fnos-feature-parity-and-update.md](./fnos-feature-parity-and-update.md)（缺口清单）、
[remote-control-device-model.md](./remote-control-device-model.md)、
[lan-remote-desktop.md](./lan-remote-desktop.md)、
[remote-workspace-identity.md](./remote-workspace-identity.md)。

## 1. 背景

fnOS/fpk 与独立 Web 部署的 `createWebPlatform()` 把 `connectRemote` 做成桩，`Root`
固定收到 `allowRemoteWorkspace=false`。服务端已有 `/api/connect-remote`（SSH/WSL/Docker）
与 `/ws/remote/:id`，但后者只暴露 file/git/system/terminal 四个服务，浏览器端也没有
把 remote attachment 注册进 `remoteWorkspaceSessionStore`，因此远程 workspace 在
Web 端不可用。

本 spec 只规划 **LAN（已配对对端）** 路径；SSH/WSL/Docker 需要先有远程资产
（server-bundle / node-pty / node-runtime）的发布源，单独决策。

## 2. 目标

在 Web/fnOS 当前窗口内：

1. 配对/列出/移除 LAN 对端（服务端持有长期令牌）；
2. 从新建连接向导选择已配对对端，挂接 `/ws/host`；
3. 选目录后像本地 workspace 一样打开 tab，任务、终端、文件树、Git、设置等
   使用远端 services；
4. 关闭 tab/断线时释放 attachment，不留下服务端 remote 连接。

## 3. 现状证据

- `packages/web/src/main.tsx`：`connectRemote`、`pairLanPeer`、`getLanPairedPeers`、
  `getLanPeerWorkspaces` 全是拒绝桩；`allowRemoteWorkspace={false}`。
- `packages/server/src/remote/connect.ts`：stdio 远程连接返回完整 `IServiceAccessor`。
- `packages/server/src/http.ts` `/ws/remote/:id`：只注册四个服务。
- `packages/desktop/src/host/lanRemoteAttach.ts`：LAN 挂接/配对在当前 desktop Host
  包里，不在 server 包；server 不能反向依赖 desktop。
- `packages/ui/src/store/remoteWorkspaceSessionStore.ts`：renderer 侧唯一 remote
  session 注册表，已导出 `registerRemoteWorkspaceSession`。

## 4. 设计

### 4.1 代码边界

- 把 `lanRemoteAttach.ts` 与 `lanPeerTransport.ts` 的**通用部分**移到
  `packages/server/src/remote/lan/`；desktop Host 改为从 `@mode/server/remote/lan/*`
  导入（保留现有行为），避免复制第二套实现。
- `packages/server` 新增 `RemoteWorkspaceSessions` 单例（进程内）：
  - `pairPeer(code, endpoint)` / `listPeers()` / `removePeer(id)`；
  - `attach({ peerId, workspacePath? })` 返回 `{ remoteSessionId, services }`；
  - `release(remoteSessionId)` 关闭 WS 并 dispose；
  - 令牌只存 Host credential 服务，不进入 HTTP 响应/日志。
- `/ws/remote/:id` 的 `ServiceCollection` 改为注册 remote accessor 的全部**已定义**
  服务（必选 + 可选存在项），并持有 connection 直到 WS close；close 时 dispose。

### 4.2 HTTP 面（沿用 `/api/*` 鉴权）

```text
GET  /api/lan/peers                 -> LanPairedPeer[]
POST /api/lan/peers/pair            -> { peer } | 401/409
DELETE /api/lan/peers/:serverId     -> { ok }
POST /api/connect-remote-lan        -> { remoteSessionId }
POST /api/remote-workspace/:id/release -> { ok }
```

请求体只接受结构化 endpoint/配对码/peerId；不接收任意 URL 或令牌。

### 4.3 Web 平台

- `connectRemote(target)`：
  1. LAN target → POST `/api/connect-remote-lan`；
  2. 用 `connectViaWebSocket(/ws/remote/<id>)` 建浏览器侧 accessor；
  3. `registerRemoteWorkspaceSession({ sessionId, target, services, dispose })`；
  4. 返回 `{ success: true, sessionId }`。SSH/WSL/Docker 维持明确 unsupported。
- `disposeRemoteSession` → `unregisterRemoteWorkspaceSession` + 关闭 WS + POST release。
- `pairLanPeer` / `getLanPairedPeers` / `getLanPeerWorkspaces` / `removeLanPairedPeer`
  映射到 HTTP 面；`allowRemoteWorkspace=true`。
- 现有 `RemoteConnectionDialog` 的 LAN 步骤与目录浏览器不改；它读取的
  `remoteWorkspaceSessionStore` 已由平台注册。

### 4.4 事件顺序

```text
向导选择已配对 LAN 对端
  → platform.connectRemote({kind:"lan", serverId})
  → POST /api/connect-remote-lan（server attach /ws/host + Initialize）
  → 浏览器 /ws/remote/<id>（完整 services）
  → registerRemoteWorkspaceSession
  → 向导目录浏览器读远端 fileService
  → 选目录 → addTab(remoteSessionId)
  → 关闭 tab / dispose → WS close → server release → 对端 attachment 释放
```

### 4.5 失败语义

| 场景 | 行为 |
| --- | --- |
| 未配对/令牌失效（401/403） | 提示重新配对；不尝试其他端点 |
| 对端不可达 | 端点探测失败 → 向导报「对端不可达」；服务端不保留 half session |
| 浏览器 WS 建立失败 | 释放 server 侧 attach；connectRemote 抛错 |
| 目录读取失败 | 保留 session，允许换目录；关闭向导则释放 |
| 页面刷新/关闭 | WS close → server release；不持久化 remote tab 的临时 connect |
| SSH/WSL/Docker | 明确返回 `unsupported`，不伪装成功 |

### 4.6 验收

1. 两台 Mode：NAS/Web 配对 LAN 对端，向导出现该设备；选目录后打开远程 tab。
2. 远程 tab 的任务、终端、文件树、Git 读取均命中远端；本地 tab 不受影响。
3. 关闭远程 tab / 断网：对端 `/ws/host` 连接关闭，NAS 无残留 remote 连接。
4. 令牌失效：向导提示重新配对，不进入半连接状态。
5. 页面刷新后远程 tab 不自动重放（reconnect 仍走既有历史链路，另行验收）。
6. 单测：配对/令牌不落响应、session release、full service 注册、失败清理。

## 5. 实现记录（2026-10-28）

- LAN 公共模块迁到 server：`packages/server/src/remote/lanRemoteAttach.ts`、
  `lanPeerTransport.ts`、`lanPeerEndpoints.ts`、`lanPairedPeers.ts`、`lanDiscovery.ts`；
  desktop 同名文件改为 `@mode/server/remote/*.js` 的兼容 re-export，行为不变。
- `LanPeerController`（`packages/server/src/remote/lanPeerController.ts`）：
  配对/列表/重命名/删除/工作区发现、attach、60s 未接管回收、对端断开回调；
  `/ws/remote/:id` 取走后由 WS close dispose。
- `createRemoteServiceCollection`（`packages/server/src/remote/remoteServiceCollection.ts`）：
  把远端 accessor 的全部已定义服务暴露给浏览器；local-only 可选服务缺失即不注册。
- `entry-http` 装配 controller，暴露 `/api/lan/peers/*`、`/api/connect-remote-lan`、
  `/api/remote-workspace/:id/release`；全部走 `/api/*` 鉴权，响应不含 token。
- Web 平台实现 `connectRemote`（LAN）、`pairLanPeer`、`getLanPairedPeers`、
  `getLanPeerWorkspaces`、`removeLanPairedPeer`、`renameLanPairedPeer`、
  `discoverLanPeers`；连接后把 services 注册进 `remoteWorkspaceSessionStore`，
  异常断线派发 `onRemoteSessionClosed`。`allowRemoteWorkspace` 在 Web Root 打开。
- `/api/server-info` 增加 `capabilities.lanPeerController`；Web 平台据此设置
  `supportsLanPeerController`，侧边栏只在服务端确实有主控面时展示远程控制页
  （桌面 LAN 静态站不展示，避免 404 空页）。
- 远程控制页在 Web/fpk 可见（设备发现/配对 + 本机配对码），远端桌面动作仍由
  `rdGetPeerCapability` 能力位隐藏。
- 已验证：服务端 LAN discovery 在真实局域网发现到 `MX121212`；配对错误码路径返回
  「配对码无效或已过期」；无令牌 401；`/ws/remote` 用 fake LAN connection 完成浏览器
  侧 `fileService.readdir` RPC，并验证 WS close 触发 dispose；全服务注册单测通过。
  **待真机**：用对端配对码完成一次配对 → 选目录 → 远程 tab 读写与关闭释放。

## 6. 非目标

- SSH/WSL/Docker 主控（需要远程资产发布源；另立 spec）。
- 远程桌面（P3；浏览器信令 + WebCodecs）。
- 多用户权限：仍由 LAN 长期令牌与 fnOS 网关登录态决定。
- 移动端窄屏的远程 tab 交互（复用现有响应式布局，不新增设计）。
