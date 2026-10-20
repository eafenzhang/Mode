# mode-agent

`mode-agent` 是面向 **Windows XP → Windows 11** 的远程被控运行时。它在一台目标机器上以单文件程序运行，
Mode 通过标准 **MCP Streamable HTTP** 或本地 CLI 发送指令，执行：

- 命令与后台作业（cmd / PowerShell / 直接启动，stdin/stdout/stderr、超时、进程树终止）
- 文件读写、目录、进程
- 截屏、鼠标、键盘、窗口置前
- ODBC 数据库查询（只读默认）

完整产品规则、安全边界与验收场景见
[`docs/specs/legacy-windows-remote-agent.md`](../../docs/specs/legacy-windows-remote-agent.md)。

## 平台与构建

| 产物 | 工具链 | 用途 |
| --- | --- | --- |
| `mode-agent-xp-x86.exe` | Go 1.10.8（最后一个官方支持 XP 的版本） | XP / Vista / 7–11 的 32 位被控端 |
| `mode-agent-win-x64.exe` | 当前 Go | x64 Windows（推荐；可看到 64 位 ODBC 驱动） |
| `mode-agent-win-x86.exe` | 当前 Go | 32 位 Windows 7–11 |
| `mode-remote-*` | 当前 Go | Mode 主机上的跨平台 CLI 客户端 |

```powershell
# Windows（默认读取仓库根 .tools/go 与 .tools/go-modern）
pwsh apps/remote-agent/build.ps1

# 或显式指定工具链
pwsh apps/remote-agent/build.ps1 -XpGo C:\go1.10.8\bin\go.exe -ModernGo go
```

```bash
# Git Bash / Linux / macOS（XP 交叉编译需要 Windows 版 Go 1.10.8）
MODE_GO_XP=/d/tools/go1.10.8/bin/go.exe MODE_GO_MODERN=go ./apps/remote-agent/build.sh
```

产物在 `apps/remote-agent/dist/`（已被 `.gitignore` 忽略）。

> XP 产物使用已 EOL 的 Go 1.10 标准库。只应部署在受控 LAN/VPN；公网暴露请使用现代
> Windows 产物 + 受信任 TLS，或反向代理/VPN。

## 一键安装（推荐）

构建产物里的 **`ModeAgent-Setup.exe`** 是唯一的安装包（32 位引导器内嵌 x86/x64 两套 agent，
运行时会按系统位数解包对应版本）：

1. 在 Mode 桌面端打开「远程控制」→「Mode Agent 被控端」→「生成配对码」，复制弹出的
   `mode-agent://enroll?...` 配对串（5 分钟内有效、一次性）。
2. 在目标机双击 `ModeAgent-Setup.exe`（XP–Win11 通用），确认安装。安装器会复制到
   `%LOCALAPPDATA%\ModeAgent`、生成 token、写登录自启动、尝试放行 TCP 47821、启动 agent，
   然后弹窗要求**输入桌面端生成的配对串**。
3. 回到桌面端点击「刷新列表」，被控端会以 HTTP MCP server 自动出现，可直接「发起对话」。

静默部署（不弹配对框，随后用 CLI 登记）：

```powershell
ModeAgent-Setup.exe install --quiet --no-firewall --no-autostart
mode-agent.exe enroll --uri "mode-agent://enroll?host=<桌面地址>&port=<桌面端口>&code=<配对码>"
# 或者显式指定安装目录（测试/定制部署）
ModeAgent-Setup.exe install --dir D:\ModeAgent --port 47821 --quiet
```

卸载：

```powershell
ModeAgent-Setup.exe uninstall --purge --yes
```

安装包和 agent 都带 Mode 应用图标。桌面端「移除被控端」只清本机 MCP 配置；需要撤销被控端
token 时在目标机执行 `install --force` 轮换。

备用方向只保留 CLI：目标机运行 `mode-agent.exe pair` 生成 8 位配对码；桌面端不再提供
「手动配对」表单，需要时用 CLI / 手动 MCP 配置。适合被控端无法反向访问桌面的网络。

## 手动配置（无安装器）

```powershell
mode-agent-xp-x86.exe init --lan --tls
mode-agent-xp-x86.exe serve --config C:\path\to\mode-agent.json
mode-agent-xp-x86.exe install-autostart --config C:\path\to\mode-agent.json
```

`init` 会打印一次 token 与 TLS 指纹。token 等价于被控端权限，请按密钥管理。

### 防火墙（按需）

```bat
:: Windows XP
netsh firewall add portopening TCP 47821 ModeAgent

:: Windows Vista+
netsh advfirewall firewall add rule name="ModeAgent" dir=in action=allow protocol=TCP localport=47821
```

## 接入 Mode（MCP）

设置 → MCP → 添加 HTTP MCP Server：

- URL：`http://<target>:47821/mcp`（TLS 时 `https://...`）
- Header：`Authorization: Bearer <token>`
- 连接后工具名前缀形如 `mcp__<serverName>__remote_exec`

可用工具：

`remote_info`、`remote_exec`、`remote_job_start/status/write/cancel/list`、
`remote_fs_list/read/write/stat/remove`、`remote_process_list/kill`、
`remote_screen_capture`、`remote_mouse`、`remote_keyboard`、
`remote_windows_list`、`remote_window_focus`、`remote_db_drivers`、`remote_db_query`。

Mode 的 HTTP MCP 客户端不做自定义证书指纹校验；自签证书请改用受信任证书、反向代理或 VPN，
或使用下面的 CLI（支持 `--tls-fingerprint`）。

## CLI

```powershell
$env:MODE_AGENT_TOKEN = "<token>"

mode-agent-xp-x86.exe remote info --host 192.168.1.20:47821
mode-agent-xp-x86.exe remote exec --host 192.168.1.20:47821 --command "ipconfig /all"
mode-agent-xp-x86.exe remote screen --host 192.168.1.20:47821 --out screen.png
mode-agent-xp-x86.exe remote mouse  --host 192.168.1.20:47821 --action click --x 640 --y 360
mode-agent-xp-x86.exe remote key    --host 192.168.1.20:47821 --text "hello"
mode-agent-xp-x86.exe remote db-query --host 192.168.1.20:47821 --dsn erp --sql "SELECT TOP 10 * FROM orders"
```

TLS + 自签证书：

```powershell
mode-agent-xp-x86.exe remote info --host 192.168.1.20:47821 --tls `
  --tls-fingerprint SHA256:<fingerprint-from-init>
```

## 配置

`mode-agent.json`（默认在可执行文件同目录；不可写时回退 `%APPDATA%\ModeAgent`）：

```json
{
  "bind": "0.0.0.0",
  "port": 47821,
  "token": "<32-byte base64url>",
  "tlsCertFile": "",
  "tlsKeyFile": "",
  "dataDir": "",
  "allowedRoots": [],
  "allowCidrs": [],
  "allowShell": true,
  "allowWrite": false,
  "allowDelete": false,
  "allowDesktopControl": false,
  "allowDb": true,
  "allowDbWrite": false,
  "dsns": {
    "erp": "DSN=ERP;UID=reader;PWD=secret"
  }
}
```

- `allowedRoots` 非空时，所有文件操作与命令 cwd 都被限制在这些根内。
- 写、删、桌面控制默认关闭；在明确需要时逐项打开。
- 数据库优先使用命名 `dsns`，避免把密码放进模型上下文。
- 审计日志：`dataDir/audit.log`，不记录命令、SQL、令牌或文件内容。

## 能力限制

- **无 PTY**：作业是管道语义，适合命令行工具与数据库客户端；全屏 TUI 不保证。
- **Session 0**：作为 Windows 服务运行时无法控制交互桌面；推荐用 `install-autostart` 以登录用户运行。
- **UIPI**：非提权 agent 无法操作高完整性窗口，也无法注入 Ctrl+Alt+Del。
- **ODBC 位数**：32 位 agent 只看到 32 位 ODBC 驱动；x64 机器优先使用 `mode-agent-win-x64.exe`。
- **无 NAT 穿透**：需要目标可达（LAN、VPN、端口映射或反向代理）。

## 开发与测试

```powershell
# Go 1.10.8（XP 兼容语法与行为）
GO111MODULE=off GOOS=windows GOARCH=386 CGO_ENABLED=0 ../../.tools/go/bin/go.exe test .

# 当前 Go
../../.tools/go-modern/go/bin/go.exe test .

# 真实 MCP 客户端联调（需要先构建 mode-agent-xp-x86.exe）
pnpm exec tsx --test apps/remote-agent/test/mcp-client.test.mts
```
