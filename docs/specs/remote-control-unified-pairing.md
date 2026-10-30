# 远程控制：统一配对码与单一设备列表

状态：2026-10-20 已拍板；2026-10-21 按
[remote-control-device-model.md](./remote-control-device-model.md) 统一设备/端点/通道模型。
关联：[legacy-windows-remote-agent.md](./legacy-windows-remote-agent.md)、
[in-app-remote-desktop.md](./in-app-remote-desktop.md)、
[lan-paired-devices.md](./lan-paired-devices.md)、
[lan-remote-attach.md](./lan-remote-attach.md)。

## 背景

远程控制页曾有三块并列入口：Mode Go 被控端（原名 Mode Agent）卡片、添加设备卡片、LAN 设备网格。
配对码也出现在两处（Mode Go 卡片与设置页），Mode Go 还保留了被控端自生成的
8 位码「手动配对」方向。用户要求：

1. 远程桌面、远程工作区与 Mode Agent 使用同一套配对码；
2. 远程控制页所有设备显示在同一个列表；
3. 修复「发起对话」不切换视图。

## 产品规则

### R1 唯一配对码

- 每个桌面实例同一时刻只有一个 LAN 一次性配对码：6 位大写十六进制、5 分钟 TTL、
  单次消费；唯一所有者 = `lanAccessServer.pairCode`。
- 同一个码同时服务：
  - `POST /api/lan/pair`：远程工作区 / 应用内远程桌面的设备接入；
  - `POST /api/remote-agent/enroll`：Mode Agent 反向登记。
- 两条路径必须走同一个 `consumePairCode`；不允许新增第二份配对码存储或第二套 TTL。
- 远程控制页只展示这一套码；不再展示被控端自生成的 8 位码，也不再提供
  「手动配对」表单。`mode-agent pair` / `/v1/pair` 仅保留为 CLI / 诊断备用，
  不进入产品 UI。
- 方向语义：本机码供其他设备接入**本机**；「添加设备」输入的是**对端**的同一类码。
  两个方向共用格式、TTL 与单次消费语义。
- 无法直连 LAN 时走 Mode Cloud 设备目录：在 Mode 账号弹窗选择账号设备并输入同一个
  6 位码；目录下发的内置中继地址自动成为公网端点，远程控制页不提供公网地址输入
  （[mode-cloud-account-directory.md](./mode-cloud-account-directory.md)）。
- 云端微信登录的 6 位十进制码与 Bot `/bind` 码**不是配对码**，只用于账号绑定；
  UI 不再用“配对码”称呼它们（[mode-cloud-account-directory.md](./mode-cloud-account-directory.md)）。

### R2 单一设备入口

**远程控制页是配对与设备管理的唯一入口**（[remote-control-device-model.md](./remote-control-device-model.md)）。
页面只消费统一 `RemoteDevice[]`，来源固定为：

1. **LAN 已配对设备**：`getLanPairedPeers()`；同一行同时代表远程工作区与远程桌面
   能力，能力位按需实时探测。地址来自 `endpoints`（公网优先、LAN 回退）。
2. **Mode Go 已配对设备（原名 Mode Agent）**：MCP 配置中的 `mode-agent-*` HTTP server；
   MCP 配置是唯一事实源，不另建设备表（内部标识不变）。
3. **发现的未配对 LAN 设备**：`discoverLanPeers()` 结果，过滤已配对 serverId 与
   用户已忽略项；行内提供对端配对码输入。
4. **云目录设备**：在 Mode 账号弹窗里选设备并输入被控端配对码；配对成功后写入
   `lan:peer:*`，回到本页设备列表，不再维护第二份云设备列表。

页面顶部区域只保留：本机配对码面板（生成/复制配对串）、添加设备（触发扫描）、刷新。
不再有独立的「Mode Go 被控端」（原名 Mode Agent）卡片、「添加设备」卡片或 LAN 设备网格分区。

新建连接向导（「添加工作区 → 选择连接方式」）是远程工作区的**唯一入口**，提供「局域网」方式，
但**只能选择已配对设备**：列表唯一来自 `getLanPairedPeers()`，不提供设备发现、手动地址/端口
或配对码输入。配对（含发现列表、输入对端配对码、重命名/删除）仍只在本页完成；
向导不得写入任何配对记录。本页设备行因此不再提供「连接工作区」按钮，避免两个工作区入口。

列表视觉：标题独占一行，副标题与右侧动作（列表/宫格、刷新、添加设备）按插件市场页
的头部样式对齐；列表模式为单卡片 + 分隔线行，宫格模式为两列卡片，动作按钮在两种模式下
都右对齐。每行两行信息（名称/状态/类型 + 地址/能力摘要），次要动作使用图标按钮并提供
`title`/`aria-label`；文案保持短句。列表/宫格偏好只存本机 localStorage。

行内动作（按 `channels` 渲染，不是按设备类型）：

| 通道 | 主操作 | 次操作 |
| --- | --- | --- |
| `workspace` | （无，工作区从新建连接向导进入） | 重新检测 / 重命名 / 删除 |
| `desktop` | 进入桌面（能力位不满足时置灰并给原因） | 重新检测 / 重命名 / 删除 |
| `conversation` | 发起对话 | 刷新 / 重命名 / 移除（两段确认） |
| 发现的未配对 | 配对（行内输码） | 忽略 |

同一台 LAN 设备在本页只保留“进入桌面”；工作区连接从新建连接向导选择该已配对设备。
公网端点对两种用途都可用。
Mode Agent 的刷新走 MCP `listWorkspaceMcpServerStatuses(mode="connect")`，更新连接状态与工具数；
重命名只写 MCP 配置里的 `modeAgent.name`，保持 serverName、URL、Authorization 不变。

`modeAgent` 元数据（`agentId` / `name` / `host` / `port` / `pairedAt` / 版本与平台）与
server 同存一份配置，属于跨 UI 与 runtime 的配置契约：CLI/runtime 的 MCP server schema
必须接受并原样保留该键。strict 校验不能因为未知元数据丢弃整个 server——否则设备从
可用的 MCP 工具面消失，「发起对话」会退化成普通本机工作区会话（表现为
`config.mcp_server.skipped`、会话 `mcpServerCount: 0`）。

空态：没有任何设备时展示统一空态；扫描无结果只在列表内提示，不新增独立卡片。

### R3 发起对话（右侧栏）

- 统一列表的「发起对话」：
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

### R4 配对地址

- 配对码面板展示本机可用地址；默认按以下优先级选首个：
  `192.168/16` → `10/8` → `172.16-31` → `100.64-127`（Tailscale/CGNAT）→ 其他 IPv4
  → `169.254` → 非 IPv4。
- 多地址时提供选择器；配对串使用所选地址 + LAN 监听端口。
- 配对串端口是桌面 LAN 监听端口（默认 45880），不是 Mode Agent 的 47821。

## 状态所有者

| 状态 | 唯一所有者 |
| --- | --- |
| 一次性配对码 | `lanAccessServer.pairCode` |
| LAN 配对记录 / 令牌 | 凭据服务（既有） |
| Mode Go 设备（原名 Mode Agent） | MCP 配置 `mode-agent-*`（既有，内部标识不变） |
| 发现的未配对设备 | `RemoteDesktopHome` 页级瞬态 |
| 设备列表 / 能力位探测 | `RemoteDesktopHome` |
| 统一 `RemoteDevice[]` 投影 | `RemoteDesktopHome`（纯投影，不持久化） |
| 远程工作区会话 | `windowRemoteConnectionRegistry`（既有） |
| 远程桌面会话 | `RemoteDesktopHostServer` / 主控 controller（既有） |
| 主视图切换 | `WorkspaceShellLayout.handleCreateTaskInChat` |

## 接口

不新增后端接口；复用：

- `getLanAccessState` / `setLanAccessEnabled` / `createLanAccessPairCode`
- `POST /api/lan/pair`、`POST /api/remote-agent/enroll`
- `getLanPairedPeers` / `pairLanPeer` / `removeLanPairedPeer` / `renameLanPairedPeer`
- `discoverLanPeers` / `rdGetPeerCapability`
- MCP store：`loadMcpFromUserDirectory` / `toggleServer` / `deleteScopedMcpServer`

## 事件顺序

```text
生成码：确保 LAN 监听开启 → createLanAccessPairCode → 展示 code + 配对串
接入：对端/agent POST 同一码 → consumePairCode → LAN 发令牌 / agent 探活并写 MCP
扫描：discoverLanPeers → 过滤已配对/已忽略 → 合并进同一列表
发起对话：确保 MCP 启用 → 有 Side Pane 宿主时打开 mode-agent-chat tab（无宿主回退 handleCreateTaskInChat）
首发：createSession(当前 workspace) → 会话加载 modeAgent server 的 MCP 工具 → tab 绑定 sessionId
```

## 验收场景

1. 同一配对码先被 LAN 设备消费后，Mode Agent 再用返回 `invalid_pair_code`；反之亦然。
2. 远程控制页只有一个设备列表，三类设备同屏；不再出现独立的 Mode Agent / 添加设备卡片。
3. 点击「发起对话」后主视图仍是远程控制页，右侧栏打开设备对话，顶部显示设备信息；
   输入框在底部且为空（无 Logo/问候），直接输入任务即可；首发后 tab 绑定到真实 session。
4. 8 位「手动配对」UI 不再出现。
5. 多地址机器默认选 `192.168/10` 地址；切换地址后配对串随之变化。
6. 已配对 Mode Agent 的 MCP 配置带 `modeAgent` 元数据时，runtime 仍加载该 server
  （会话 `mcpServerCount > 0`，无 `config.mcp_server.skipped`）；设备对话里的请求能调用
  该设备的 MCP 工具，而不是只由本机工作区 Agent 回答。
7. `pnpm typecheck`、`pnpm lint` 与相关 UI/desktop 测试通过。

## 不做

- 不改 LAN 配对协议、令牌格式、远程桌面会话协议。
- 不移除 Go agent 的 `pair` / `/v1/pair` CLI 能力（仅从产品 UI 隐藏）。
- 不把 SSH/WSL/Docker 的凭据表单并入本页；它们继续由新建连接向导创建，但创建后
  的会话与 LAN 远程工作区共用同一 registry 与 attachment 生命周期。
