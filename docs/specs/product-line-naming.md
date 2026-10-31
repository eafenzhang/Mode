# 产品线与命名（Mode 生态）

状态：2026-10-30 拍板。本文是用户可见产品名的唯一事实源；新增/改名先改本文再改代码。

## 产品线

| 产品名 | 定位 | 状态 | 仓库/内部标识 |
| --- | --- | --- | --- |
| **Mode Cloud** | 云服务：个人账号（GitHub/用户名密码）、设备目录、WSS 中继、网页门户 | 已上线（fnOS 自部署，`relay.mymode.cloud`） | `packages/cloud-relay`；容器 `mode-cloud-relay` |
| **Mode** | 编程智能体桌面端 | 现役 | `packages/desktop`、`apps/desktop-tauri`、`mode` CLI |
| **Mode Go** | 智能体桌面被控端（远控目标、MCP 接入） | 现役 | 原名「Mode Agent 被控端」；内部 origin `mode-agent`、MCP 前缀 `mode-agent-*` 不变 |
| **Mode Bot** | 对标 Grok Bot | 待开发（名称预留）；产品 spec 草案见 [mode-bot.md](./mode-bot.md) | 暂无实现 |

## 命名规则

1. **用户可见文案只使用上表产品名**，大小写固定：`Mode Cloud`、`Mode`、`Mode Go`、
   `Mode Bot`（不写 Mode GO / ModeGo；英文界面同样按此大小写）。
2. **内部标识不随产品名改**：`mode-agent` 作为 `RemoteDeviceOrigin` 的取值、MCP server
   前缀 `mode-agent-*`、类/文件名（`useModeAgentDevices`、`modeAgentPairing` 等）与
   i18n key 名（`remoteDesktop.modeAgents.*`）全部保留，避免破坏已配对设备、MCP 配置
   与协议兼容性。改名只影响展示层与文档措辞。
3. **「Mode Agent」只保留在智能体运行时语境**（MCP 设置、子智能体、命令来源、
   model-io 等），不得再用于称呼被控端设备；被控端统一称 **Mode Go**。
4. **Mode Bot 尚未实现**：只允许在路线图/命名文档中出现；实现前先补产品 spec
   （目标、状态所有者、接口与验收），不得先落 UI 文案承诺。
5. 云服务相关文案统一用「Mode Cloud」；账号入口（左下角、模式账号弹窗、网页门户）
   与云服务概念区分：门户标题为 Mode Cloud，不含其他产品名。

## 影响面与验收

1. 中文/英文界面：远控页分区标题、设备类型徽标、空状态、页面说明与配对码说明使用
   「Mode Go」；不再出现「Mode Agent 被控端」。
2. 「Mode Agent」在界面中仅存于智能体运行时语境（MCP/子智能体/命令来源等）。
3. `docs/specs` 中的设备模型、统一配对、应用内远控等 spec 使用统一产品名，
   并标注内部标识不变的说明。
4. 新发布物（fpk、桌面安装包、官网文案）不得引入未定义的产品名；Mode Bot 在实现前
   不出现于产品界面。
