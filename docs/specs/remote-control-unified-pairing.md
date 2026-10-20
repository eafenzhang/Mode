# 远程控制：统一配对码与单一设备列表

状态：2026-10-20 已拍板，实现与验收随本次变更交付。关联：
[legacy-windows-remote-agent.md](./legacy-windows-remote-agent.md)、
[in-app-remote-desktop.md](./in-app-remote-desktop.md)、
[lan-paired-devices.md](./lan-paired-devices.md)、
[lan-remote-attach.md](./lan-remote-attach.md)。

## 背景

远程控制页曾有三块并列入口：Mode Agent 被控端卡片、添加设备卡片、LAN 设备网格。
配对码也出现在两处（Mode Agent 卡片与设置页），Mode Agent 还保留了被控端自生成的
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
- 无法直连 LAN 时，远程控制页提供「公网地址 + 配对码」入口：仍调用同一个
  `pairLanPeer`、消费同一个码，不新增第二套码或第二份 TTL；公网地址必须是 HTTPS
  （[lan-remote-public-endpoints.md](./lan-remote-public-endpoints.md)）。

### R2 单一设备列表

远程控制页只有一个设备列表，行类型固定为三类：

1. **LAN 已配对设备**：`getLanPairedPeers()`；同一行同时代表远程工作区与远程桌面
   能力，能力位按需实时探测。
2. **Mode Agent 已配对设备**：MCP 配置中的 `mode-agent-*` HTTP server；MCP 配置是
   唯一事实源，不另建设备表。
3. **发现的未配对 LAN 设备**：`discoverLanPeers()` 结果，过滤已配对 serverId 与
   用户已忽略项；行内提供对端配对码输入。

页面顶部区域只保留：本机配对码面板（生成/复制配对串）、添加设备（触发扫描）、刷新。
不再有独立的「Mode Agent 被控端」卡片、「添加设备」卡片或 LAN 设备网格分区。

列表视觉：标题独占一行，副标题与右侧动作（列表/宫格、刷新、添加设备）按插件市场页
的头部样式对齐；列表模式为单卡片 + 分隔线行，宫格模式为两列卡片，动作按钮在两种模式下
都右对齐。每行两行信息（名称/状态/类型 + 地址/能力摘要），次要动作使用图标按钮并提供
`title`/`aria-label`；文案保持短句。列表/宫格偏好只存本机 localStorage。

行内动作：

| 类型 | 主操作 | 次操作 |
| --- | --- | --- |
| LAN 已配对 | 进入桌面（能力位不满足时置灰并给原因） | 重新检测 / 重命名 / 删除 |
| Mode Agent | 发起对话 | 刷新 / 重命名 / 移除（两段确认） |
| 发现的未配对 | 配对（行内输码） | 忽略 |

Mode Agent 的刷新走 MCP `listWorkspaceMcpServerStatuses(mode="connect")`，更新连接状态与工具数；
重命名只写 MCP 配置里的 `modeAgent.name`，保持 serverName、URL、Authorization 不变。

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
| Mode Agent 设备 | MCP 配置 `mode-agent-*`（既有） |
| 发现的未配对设备 | `RemoteDesktopHome` 页级瞬态 |
| 设备列表 / 能力位探测 | `RemoteDesktopHome` |
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
发起对话：确保 MCP 启用 → handleCreateTaskInChat → showChatMainView → startDraft + 预填
```

## 验收场景

1. 同一配对码先被 LAN 设备消费后，Mode Agent 再用返回 `invalid_pair_code`；反之亦然。
2. 远程控制页只有一个设备列表，三类设备同屏；不再出现独立的 Mode Agent / 添加设备卡片。
3. 点击「发起对话」后主视图仍是远程控制页，右侧栏打开设备对话，顶部显示设备信息；
   输入框在底部且为空（无 Logo/问候），直接输入任务即可；首发后 tab 绑定到真实 session。
4. 8 位「手动配对」UI 不再出现。
5. 多地址机器默认选 `192.168/10` 地址；切换地址后配对串随之变化。
6. `pnpm typecheck`、`pnpm lint` 与相关 UI/desktop 测试通过。

## 不做

- 不改 LAN 配对协议、令牌格式、远程桌面会话协议。
- 不移除 Go agent 的 `pair` / `/v1/pair` CLI 能力（仅从产品 UI 隐藏）。
- 不把远程工作区连接向导并入本页；本页只统一配对码与设备列表。
