# Spec：Web / 手机作为 SSH / Docker 远程工作区主控

状态：2026-10-28 立项，M1（控制面与 Web 接线）实现中。关联：
[web-remote-workspace.md](./web-remote-workspace.md)（Web LAN 主控）、
[remote-control-device-model.md](./remote-control-device-model.md)（设备模型）、
[fnos-feature-parity-and-update.md](./fnos-feature-parity-and-update.md)（fpk 能力矩阵）。

## 1. 问题

`@mode/server/remote` 已有完整的 SSH / WSL / Docker 后端、部署与握手
（`createRemoteBackend` + `connectRemote`），桌面 Host 也在用；`packages/server/src/http.ts`
甚至已暴露 `POST /api/connect-remote` 与 `/ws/remote/:id`。但：

- Web 平台 `connectRemote` 对非 LAN 目标直接返回 `unsupported`，向导里的 SSH/Docker
  入口在 Web 上点了必失败；
- `listSSHConfigAliases` / `listDockerContainers` / `isDockerAvailable` 在 Web 是空桩，
  向导无法列出现有 SSH 别名与容器；
- `/api/connect-remote` 没有 remote assets 源解析，也没有「浏览器未接管」的回收：
  POST 成功后不连 `/ws/remote/:id` 的连接会永久留在内存里；
- 桌面 LAN 访问服务（手机浏览器连桌面）只提供被控/工作区服务，没有把设备控制面
  （LAN 配对、远程桌面中继、SSH/Docker 发起）暴露给手机。

## 2. 范围

**M1（本 spec 的验收范围）**

1. Web/fpk 的远程连接向导支持 **SSH** 与 **Docker**；WSL 仍只在 Windows 桌面端出现
   （`buildAvailableKinds` 依据 `isWindowsDesktop`，Web 恒 false）。
2. `POST /api/connect-remote` 在服务端解析 remote assets 源并传给 `connectRemote`；
   缺少资产源时在发起前返回稳定错误，不进入半截部署。
3. 浏览器 POST 成功后必须在 60s 内接管 `/ws/remote/:id`，否则服务端释放该连接；
   同时最多 4 条待接管连接。
4. 新增服务端环境探测：SSH 别名列表（`~/.ssh/config`，服务器本机）、Docker 容器列表
   （服务器本机 `docker ps`）。
5. 桌面 LAN 访问服务装配 `LanPeerController` 与 `RemoteDesktopPeerController`：
   手机浏览器经桌面 Host 也能使用 LAN 设备页与远程桌面主控；SSH/Docker 由既有
   `/api/connect-remote` 提供，不需要新能力位。

**非目标（另行立项）**

- remote assets 的**发布/托管**：M1 只消费 `MODE_REMOTE_ASSET_CDN_BASE_URL`
  （含旧名回退）或 `MODE_MOCK_CDN_DIR`；把 `prepare:remote-assets` 产物随 fpk 发布或
  上传到 release CDN 属于下一阶段（见 §9）。
- WSL 主控（NAS 是 Linux，没有 WSL）。
- SSH 主机密钥的交互式信任（沿用 `@mode/server/remote` 现状；不新增 TOFU UI）。
- 远程会话断线重连（沿用 Web 的 `web-remote-replayable` + 历史重连链路）。

## 3. 产品规则

- **入口**：新建连接向导的 SSH / Docker 步骤在 Web 与桌面一致；不新增独立页面。
  私有密钥路径是**服务器本机路径**（NAS 上的文件），Web 的文件选择按钮不可用，
  但输入框允许手输绝对路径。
- **凭据**：SSH 密码/私钥口令只存在于 `POST /api/connect-remote` 请求体与服务端
  内存中的 backend；不写凭据服务、不写日志、不回显到响应。连接释放即消失。
- **资产源**：`resolveRemoteAssetDirs()` 按 `MODE_MOCK_CDN_DIR` > `MODE_REMOTE_ASSET_CDN_BASE_URL`
  （含 `ZCODIUM_`/`ZCODE_` 旧名）解析；缓存目录固定 `{config}/remote-assets-cache`。
  两者都没有时返回 `409 { error: "remote_assets_unavailable" }`，文案沿用桌面
  「缺少 remote assets 源」的说明。
- **SSH 别名**：只读服务器 `~/.ssh/config`；用户点选别名后由向导填充 host/port/user。
  别名不可用时返回空列表，不阻塞手动输入。
- **Docker**：`available=false` 时不展示容器下拉（仍允许手输容器名），与桌面语义一致。
- **桌面 LAN 控制面**：手机经桌面 LAN 服务连接后，能力位
  `capabilities.lanPeerController=true`；`/api/connect-remote` 始终可用（受设备令牌保护）。

## 4. 所有者

| 状态 | 所有者 | 位置 |
| --- | --- | --- |
| 待接管远程连接 | 服务端 `PendingRemoteConnections` | 进程内 Map（60s TTL / 最多 4 条） |
| 已接管浏览器会话 | 浏览器 `remoteWorkspaceSessionStore` | `remoteWorkspaceSockets` + store |
| SSH/Docker 后端与部署 | `@mode/server/remote`（既有） | 连接生命周期内 |
| remote assets 源 | 启动环境变量（只读） | `MODE_MOCK_CDN_DIR` / `MODE_REMOTE_ASSET_CDN_BASE_URL` |
| LAN 设备/远程桌面主控 | `LanPeerController` / `RemoteDesktopPeerController` | 服务端进程内 |

## 5. 协议

```text
GET  /api/remote-environments/ssh-aliases
  -> { aliases: SSHConfigAliasOption[] }            // 读不到 = []

GET  /api/remote-environments/docker
  -> { available: boolean, containers: DockerContainerInfo[] }
     // docker 不存在/daemon 未启动/命令失败 → { available:false, containers:[] }

POST /api/connect-remote
  body: RemoteTarget（ssh | docker | wsl）
  -> { id }                                          // 200，浏览器需在 60s 内接管
  -> 409 { error: "remote_assets_unavailable" }      // 未配置资产源
  -> 503 { error: "too_many_pending_connections" }   // 待接管已满
  -> 500 { error: string }                           // 后端/部署失败

WS   /ws/remote/:id                                  // 既有，取出即删；close 释放连接
```

- 所有端点走既有 `/api/*`、`/ws/*` 令牌鉴权，未授权 401。
- `POST /api/connect-remote` 不接收任意 URL；目标只来自 `remoteTargetSchema`。

## 6. 事件顺序

```text
向导选择 SSH/Docker → 填参数 → 下一步
  → platform.connectRemote(target)
  → POST /api/connect-remote
      ├─ 无资产源 → 409，向导显示可读错误
      └─ createRemoteBackend → connectRemote(deploy/handshake) → pending.add(id)
  → 浏览器 connectViaWebSocket(/ws/remote/<id>)
      └─ server: pending.take(id) → setupChannelServer(...)
  → registerRemoteWorkspaceSession → 目录浏览器 → 打开远程 tab
关闭 tab / 向导取消 / 页面关闭
  → browser WS close → server disposeConnection
  → pending TTL 兜底：未接管连接 60s 后 dispose
```

## 7. 失败语义

| 场景 | 行为 |
| --- | --- |
| 未配置 remote assets 源 | `409 remote_assets_unavailable`，不创建 backend/不部署 |
| SSH 认证失败/主机不可达 | `500` + `normalizeSSHConnectError` 的可读文案 |
| Docker 容器不存在/未运行 | `500` + docker 错误；不留下 pending 条目 |
| 待接管连接超时 | 60s 后 dispose，浏览器再连得到 4004 |
| 待接管达到上限 | `503 too_many_pending_connections`，已建立的 backend 立即 dispose |
| 浏览器 WS 建立失败 | connectRemote 返回错误；服务端条目由 TTL 回收 |
| 页面刷新 | WS close → dispose；不重放半连接 |
| SSH 别名/Docker 探测失败 | 返回空结果/available:false，不阻塞手动输入 |

## 8. 安全

- 目标 schema 只允许结构化字段；服务端不接受任意 URL/命令。
- SSH 密码/口令只经 HTTPS（网关）或本地 LAN 传输，仅存内存；错误信息不得包含密码。
- 本机命令执行面：`docker ps`、读取 `~/.ssh/config`，仅限已鉴权请求。
- 桌面 LAN 控制面复用逐设备令牌；未配对设备 401。
- 不把对端长期令牌或私钥写进响应、日志或凭据服务。

## 9. 后续阶段：remote assets 发布

M1 之后需要二选一（另行立项）：

1. **Release CDN**：CI 在发布 tag 时执行 `prepare:remote-assets --platforms linux-x64`，
   把 `releases/<version>/manifest-linux-x64.json` 与 `components/linux-x64/*` 上传到
   Release/对象存储，构建期把 `MODE_REMOTE_ASSET_CDN_BASE_URL` 注入 server/Host。
2. **随 fpk 自带**：把 linux-x64 组件（约 56 MiB 压缩）打进 fpk 的 `remote-assets/`，
   `cmd/main` 注入 `MODE_MOCK_CDN_DIR=$TRIM_APPDEST/remote-assets`，安装即可用，
   代价是安装包体积增加。

## 10. 验收

1. Web/fpk 向导出现 SSH 与 Docker；WSL 不出现。
2. 未配置资产源时点下一步：立即得到「缺少 remote assets 源」错误，不产生待接管连接。
3. 配置 `MODE_MOCK_CDN_DIR`（指向 `prepare:remote-assets` 产物）后：SSH 连到 Linux
   主机 → 选目录 → 远程 tab 的终端/文件树可用；关闭 tab 后远端进程退出。
4. Docker：`available=true` 时列出容器；选中运行中容器可连接。
5. SSH 别名列表来自服务器 `~/.ssh/config`；点选后 host/port/user 正确填充。
6. POST 后不连 `/ws/remote/:id`：60s 后条目释放；并发第 5 条返回 503。
7. 手机连接桌面 LAN 服务：设备页可配对/查看 LAN 设备，可发起远程桌面；
   SSH/Docker 向导可用（受桌面 Host 的资产源配置约束）。
8. 单元测试覆盖 pending TTL/上限/接管/全量释放、环境端点鉴权与空结果、
   `/api/connect-remote` 的资产源缺失与 schema 校验。

## 11. 实现记录（2026-10-28，M1）

- 新增 `packages/server/src/remote/remoteAssetSource.ts`：按
  `MODE_MOCK_CDN_DIR` / `MODE_HOST_RESOURCES_DIR/mock-cdn` / 开发态
  `packages/desktop/mock-cdn` / `MODE_REMOTE_ASSET_CDN_BASE_URL`（含旧名）解析；
  桌面 Host 把 `resolveRemoteAssetDirs()` 的结果经 `startLanAccessServer` →
  `createHttpServer` 传入，手机与桌面 renderer 使用同一份资产源。
- 新增 `PendingRemoteConnections`（60s TTL / 最多 4 条 / `disposeAll`），
  `/api/connect-remote` 与 `/ws/remote/:id` 改用它；HTTP server `close` 时全量释放。
- `/api/connect-remote`：拒绝 LAN 目标（走 `/api/connect-remote-lan`）；缺资产源返回
  `409 remote_assets_unavailable`；成功时把 `appVersion`、runtime env 与资产目录传给
  `connectRemote`。
- 新增 `GET /api/remote-environments/ssh-aliases`（服务器 `~/.ssh/config`）与
  `GET /api/remote-environments/docker`（`docker version` + `docker ps`）；失败空结果。
- Web 平台 `connectRemote` 支持 SSH/Docker：POST → 接管 `/ws/remote/:id` → 注册
  session；`listSSHConfigAliases` / `listDockerContainers` / `isDockerAvailable`
  改为真实端点（5s 短缓存避免同轮表单重复 `docker ps`）。
- 桌面 LAN 访问服务装配 `LanPeerController` + `RemoteDesktopPeerController`：
  手机浏览器获得 LAN 设备页、配对与远程桌面主控能力；`dispose` 时先释放控制器再关服务。
- 测试：`pendingRemoteConnections.test.ts`（TTL/上限/take/disposeAll）、
  `remoteEnvironments.test.ts`（鉴权、空结果、schema、LAN 拒绝、缺资产源 409）。
- **未完成**：remote assets 的发布/自带（§9），因此真实 SSH/Docker 连接仍要求部署侧
  配置资产源；真机 SSH/Docker 连接与手机经桌面 LAN 的端到端验收待执行。
