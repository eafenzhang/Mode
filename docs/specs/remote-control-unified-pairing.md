# 远程控制：统一设备列表（配对码已从产品 UI 移除）

状态：2026-10-20 已拍板；2026-10-21 按
[remote-control-device-model.md](./remote-control-device-model.md) 统一设备/端点/通道模型；
**2026-11-13 修订**：产品统一个人账号体系后，配对码与配对入口从桌面端产品 UI 全部移除，
远程控制页只消费同账号 v3 设备目录。原「统一配对码」规则仅保留后端与 CLI 语义（见 R1）。
关联：[legacy-windows-remote-agent.md](./legacy-windows-remote-agent.md)、
[in-app-remote-desktop.md](./in-app-remote-desktop.md)、
[lan-paired-devices.md](./lan-paired-devices.md)、
[lan-remote-attach.md](./lan-remote-attach.md)、
[remote-control-v3.md](./remote-control-v3.md)。

## 背景

远程控制页曾有三块并列入口：Mode Go 被控端（原名 Mode Agent）卡片、添加设备卡片、LAN 设备网格。
配对码也出现在两处（Mode Go 卡片与设置页），Mode Go 还保留了被控端自生成的
8 位码「手动配对」方向。用户要求：

1. 远程桌面、远程工作区与 Mode Agent 使用同一套配对码；
2. 远程控制页所有设备显示在同一个列表；
3. 修复「发起对话」不切换视图。

## 产品规则

### R1 配对码（后端保留，产品 UI 已移除）

- 每个桌面实例同一时刻只有一个 LAN 一次性配对码：6 位大写十六进制、5 分钟 TTL、
  单次消费；唯一所有者 = `lanAccessServer.pairCode`。
- 同一个码仍服务两条**非产品 UI** 路径：
  - `POST /api/lan/pair`：局域网浏览器/旧客户端的令牌换取（`?token=` 链接之外的兼容口）；
  - `POST /api/remote-agent/enroll`：Mode Agent 反向登记。
- 两条路径必须走同一个 `consumePairCode`；不允许新增第二份配对码存储或第二套 TTL。
- **2026-11-13 起**：桌面端远程控制页与远程控制设置页不再展示/输入配对码，
  也不再提供「扫描发现 + 配对」。`mode-agent pair` / `/v1/pair` 与上述 HTTP 端点
  仅保留为 CLI / 诊断 / 旧客户端兼容，不得再挂回产品 UI。
- 同账号设备之间不需要任何码：设备目录（`remoteV3Service.listHosts()`）即授权来源，
  见 R2。

### R2 单一设备入口

**远程控制页只展示同账号 v3 Host**（[remote-control-v3.md](./remote-control-v3.md)）。
数据来源唯一：`remoteV3Service.getState()` 判定是否已登录，`remoteV3Service.listHosts()`
给出目录（Host 角色、已排除本机）。页面状态四态：未登录 / 加载中 / 已加载（含空列表）/ 加载失败。

- 设备行只有一种：v3 Host。展示在线状态、名称、端到端加密徽标、地址（`addresses[0]`
  或 deviceId 前 8 位），动作只有「进入桌面」。离线或本机正被控时置灰并给原因。
- 不提供重命名/删除/重新检测：v3 目录没有这些远端写操作，本机也不缓存第二份设备表。
- 不提供能力位探测（`rdGetPeerCapability` 是 LAN 配对链路语义，对 v3 deviceId 无意义）；
  在线状态即目录给出的 Control 在线状态，实际可连性由会话面在建连时权威判定。
- **不再出现**：本机配对码面板、添加设备/扫描入口、发现的未配对设备行、Mode Agent 设备行、
  LAN 已配对设备行。后端配对与登记能力按 R1 保留，但不进入产品 UI。

新建连接向导（「添加工作区 → 选择连接方式」）仍是远程工作区的**唯一入口**。
2026-11-13 起向导的「局域网」方式也已下线：不再读取 `getLanPairedPeers()`，不再向用户
展示已配对 LAN 设备，连接方式只剩 SSH / WSL（Windows 桌面）/ Docker / 远程控制（v3）。
历史 LAN 远程工作区仍可按持久化 target 重连，但产品 UI 不再产生新的 LAN 工作区。
本页设备行不提供「连接工作区」按钮。

列表视觉：标题独占一行，描述与右侧动作（列表/宫格、刷新）按插件市场页的头部样式对齐；
列表模式为单卡片 + 分隔线行，宫格模式为两列卡片，动作按钮在两种模式下都右对齐。
每行两行信息（名称/状态/类型 + 地址/加密摘要），次要动作使用图标按钮并提供
`title`/`aria-label`；文案保持短句。列表/宫格偏好只存本机 localStorage。

行内动作固定为一条：

| 设备 | 主操作 | 次操作 |
| --- | --- | --- |
| 同账号 v3 Host | 进入桌面（离线或本机被控中置灰并给原因） | 无 |

空态分两种：未登录（引导「登录账号」+「打开远程控制设置」）；已登录但目录为空
（引导「打开远程控制设置」+「刷新」）。加载失败必须显式给出错误与重试，不得吞成空列表。

### R3 发起对话（右侧栏，随设备页一起下线）

- **2026-11-13 起**：远程控制页不再列出 Mode Agent 设备，页面上的「发起对话」入口随之移除；
  `ModeAgentChatSidePane` 与 `mode-agent-chat` tab 机制本身保留（已恢复的 tab 仍可显示），
  但不再由远程控制页创建新对话。
- 下文为历史规则，保留用于已存在 tab 的行为约束：
  1. 若 MCP server 未启用，先启用并落盘；
  2. 在右侧 Side Pane 打开一个 `mode-agent-chat` tab，**主视图保持远程控制页**；
  3. 面板顶部固定设备信息（名称、`host:port`、MCP server），下面复用 `SessionPane`
     的草稿能力；草稿不预填远程指令，输入框固定在底部，空态不显示 Logo/问候，
     placeholder 为「输入任务或指令」，用户直接输入任务。
- 一个 workspace 只保留一个设备对话 tab：切换设备时重置为草稿；同一设备已有 session
  时只聚焦，不覆盖会话。
- 首发成功后 `onSessionCreated` 把 tab 从 `sessionId=null` 绑定到真实 session；离开
  远程控制页时关闭仍未首发的草稿 tab，已绑定 session 的 tab 保留（它不再与主对话
  共用 workspace 草稿）。
- 没有 Side Pane 宿主的上下文（如 Web）回退到 `handleCreateTaskInChat`，行为回到
  「切主视图 + 建草稿」。

### R4 配对地址（随配对面板下线）

- **2026-11-13 起**：配对码面板不再出现在产品 UI，本节仅约束仍需展示本机地址的诊断面
  （CLI `mode-agent pair`、`/api/lan/state` 返回值）。
- 地址优先级仍按：`192.168/16` → `10/8` → `172.16-31` → `100.64-127`（Tailscale/CGNAT）
  → 其他 IPv4 → `169.254` → 非 IPv4。
- 配对串端口是桌面 LAN 监听端口（默认 45880），不是 Mode Agent 的 47821。

## 状态所有者

| 状态 | 唯一所有者 |
| --- | --- |
| 一次性配对码 | `lanAccessServer.pairCode`（仅后端/CLI，无产品 UI） |
| LAN 配对记录 / 令牌 | 凭据服务（既有，兼容历史记录） |
| Mode Go 设备（原名 Mode Agent） | MCP 配置 `mode-agent-*`（既有，内部标识不变） |
| 同账号 v3 Host 目录 | `remoteV3Service.listHosts()`（账号服务） |
| 远程控制页列表状态（未登录/加载/ok/error） | `RemoteDesktopHome`（纯投影，不持久化） |
| 本机被控状态 | `rdGetControlledState()` |
| 远程工作区会话 | `windowRemoteConnectionRegistry`（既有） |
| 远程桌面会话 | `RemoteDesktopHostServer` / 主控 controller（既有） |
| 主视图切换 | `WorkspaceShellLayout.handleCreateTaskInChat` |

## 接口

远程控制页只新增「读」依赖：

- `remoteV3Service.getState()` / `remoteV3Service.listHosts()`
- `rdGetControlledState()`

后端保留、产品 UI 不再调用：

- `createLanAccessPairCode`、`POST /api/lan/pair`、`POST /api/remote-agent/enroll`
- `getLanPairedPeers` / `pairLanPeer` / `removeLanPairedPeer` / `renameLanPairedPeer`
- `discoverLanPeers` / `rdGetPeerCapability`
- MCP store：`loadMcpFromUserDirectory` / `toggleServer` / `deleteScopedMcpServer`

## 事件顺序

```text
进入页面：getState → 未登录则空态(登录账号)；已登录 → listHosts → 列表/空目录/失败
刷新/轮询：listHosts（15s，页面可见且不在会话中）
进入桌面：sessionTarget → RemoteDesktopSession（建连时由会话面权威判定可连性）
退出会话：回列表并重读 rdGetControlledState
（历史/CLI）生成码 → createLanAccessPairCode；对端 POST 同一码 → consumePairCode → 发令牌
```

## 验收场景

1. 配对码与「扫描发现」不再出现在远程控制页与远程控制设置页；`mode-agent pair` 与
   `/api/lan/pair` 仍可用（CLI/旧客户端）。
2. 远程控制页只有一个列表，行只有「进入桌面」；离线或本机被控中置灰并给原因。
3. 未登录、已登录无 Host、加载失败三种空/错误态都能明确区分，加载失败有重试。
4. 设置页只剩「允许局域网直连」「允许被远程控制」两个开关与编码能力/安全提示。
5. `pnpm typecheck`、`pnpm lint` 与相关 UI/desktop 测试通过。

## 不做

- 不改 LAN 配对协议、令牌格式、远程桌面会话协议。
- 不移除 Go agent 的 `pair` / `/v1/pair` CLI 能力（仅从产品 UI 隐藏）。
- 不把 SSH/WSL/Docker 的凭据表单并入本页；它们继续由新建连接向导创建，但创建后
  的会话与 LAN 远程工作区共用同一 registry 与 attachment 生命周期。
