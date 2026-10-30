# mode-agent

`mode-agent` 是面向 **Windows XP → Windows 11** 的远程被控运行时。它在一台目标机器上以单文件程序运行，
Mode 通过标准 **MCP Streamable HTTP** 或本地 CLI 发送指令，执行：

- 命令与后台作业（cmd / PowerShell / 直接启动，stdin/stdout/stderr、超时、进程树终止）
- 文件读写、目录、进程
- 截屏、鼠标、键盘、窗口置前
- ODBC 数据库查询（只读默认）
- 本地面板：固定 300×400，默认显示 Matrix 风格快速代码雨；内容由 Mode 桌面端通过 MCP/HTTP 控制
- 托盘、开机自启、崩溃/被杀自动重启

完整产品规则、安全边界与验收场景见
[`docs/specs/mode-agent-tray-panel.md`](../../docs/specs/mode-agent-tray-panel.md) 与
[`docs/specs/legacy-windows-remote-agent.md`](../../docs/specs/legacy-windows-remote-agent.md)。

## 平台与构建

| 产物 | 工具链 | 用途 |
| --- | --- | --- |
| `mode-agent-xp-x86.exe` | Go 1.10.8（最后一个官方支持 XP 的版本） | XP / Vista / 7–11 的 32 位被控端 |
| `mode-agent-win-x64.exe` | 当前 Go | x64 Windows（推荐；可看到 64 位 ODBC 驱动） |
| `mode-agent-win-x86.exe` | 当前 Go | 32 位 Windows 7–11 |
| `mode-remote-*` | 当前 Go | Mode 主机上的跨平台 CLI 客户端 |
| `ModeGo-Setup.exe` | Go 1.10 引导器 + 当前 Go 载荷 | 可选的安装/卸载形态 |

Windows 产物均为 **GUI 子系统、单文件、无运行时依赖、带 Mode 图标**；双击不会出现控制台窗口。

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

产物在 `apps/remote-agent/dist/`（已被 `.gitignore` 忽略）。图标由 `set-icon.ps1` 直接写入 PE
资源，不依赖 rcedit。

> XP 产物使用已 EOL 的 Go 1.10 标准库。只应部署在受控 LAN/VPN；公网暴露请使用现代
> Windows 产物 + 受信任 TLS，或反向代理/VPN。

## 免安装运行（推荐）

1. 把 `mode-agent-win-x64.exe`（或 x86 / XP 版）复制到目标机，双击。
2. 首次运行对话框选择是否**开机自启**、**显示托盘图标**；本地面板固定为 300×400。
   确认后在 exe 同目录生成 `mode-agent.json`。目录不可写时回退到
   `%APPDATA%\ModeAgent\mode-agent.json`。
3. 启动后出现固定的 300×400 面板（默认 Matrix 代码雨）和托盘图标，并自动弹出 **GitHub 登录**：
   浏览器打开 `github.com/login/device`，输入面板上的 user code 完成授权；授权后设备自动
   加入你的 Mode Cloud 账号设备目录（无需桌面端配对码）。
4. 在桌面端登录**同一账号**，「远程控制」页即可看到这台设备，点「连接」直连。
5. 之后点击面板标题栏左侧的 Mode 图标，打开 Windows 原生系统菜单（还原/移动/关闭 等），
   菜单末尾是「设置...」「登录 GitHub...」「退出 Mode Go」；双击托盘图标只负责打开面板。

面板无内容时默认显示 Matrix 风格快速代码雨；Mode 桌面端通过 MCP 工具
`remote_panel_show` / `remote_panel_clear` / `remote_panel_status`（或直接调用 `/v1/panel`）
控制内容。推送内容后切换为内容视图，清空后动画恢复。支持纯文本与 PNG/JPEG，不渲染 HTML。

### 设置 / 登录（免安装 exe 内置）

免安装 exe 自带独立的设置页和 GitHub 登录入口，不需要再运行 `ModeGo-Setup.exe`：

- 入口：点击面板标题栏左侧的 Mode 图标（或 Alt+Space）打开原生系统菜单，末尾是
  「设置...」「登录 GitHub...」「退出 Mode Go」。
- **设置页**只包含两个开关：**登录 Windows 时自动启动**、**显示系统托盘图标**。
  保存后立即生效，不需要重启；本地面板固定 300×400、始终启用，不出现在设置里。
  - 关闭窗口只隐藏面板；双击托盘图标或重新运行 exe 可再次打开。
  - 关闭托盘会移除通知区图标；面板仍可通过运行 exe 打开。
  - 自启开关同步写入/删除 `HKCU\...\Run\ModeAgent`。
- **登录入口**触发 `mode-agent.exe login`：GitHub Device Flow（显示 user code、自动打开
  授权页），授权后自动 enroll 到个人账号目录。设备换机/重装后重新 login 即可。
  - 云端地址可用 env `MODE_CLOUD_BASE_URL` 覆盖（私有化部署）。

### 配置文件与运行文件

配置与日志都在同一个数据目录（默认 exe 同目录；不可写时 `%APPDATA%\ModeAgent`）：

- `mode-agent.json`：配置（token、端口、能力开关、`panel` 设置）
- `agent.pid`：agent 子进程 PID
- `agent.log`：agent 标准输出/错误
- `supervisor.log`：守护进程的启停、崩溃重启与错误记录
- `mode-agent.stop`：主动退出标记；存在时守护进程不再重启

### 开机自启

- 设置页里的「登录 Windows 时自动启动」立即写入/删除
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\ModeAgent`。
- Run 项命令为 `"<exe>" start --config "<config>"`，直接启动 GUI exe，不经过 `cmd.exe`，
  登录时不会闪命令行窗口。
- 移动 exe 后自启项会失效；重新运行并在托盘中重开自启即可。
- CLI：

```powershell
mode-agent.exe install-autostart --config C:\path\to\mode-agent.json
mode-agent.exe uninstall-autostart
```

### 守护与退出

`start` 是守护入口：它拉起 `serve --ui` 子进程，每 2 秒探测 `/health` 并用 token 校验
`/v1/info`。子进程崩溃、被 Task Manager 结束或异常退出后会自动重启；启动时带 `--open-panel`
则重启后重新显示面板。

```powershell
mode-agent.exe start --config C:\path\to\mode-agent.json --open-panel
mode-agent.exe stop  --config C:\path\to\mode-agent.json
```

- 托盘「退出 Mode Go」、`/v1/shutdown`、`stop` 会先写 `mode-agent.stop`，supervisor 看到后
  不再重启并退出。
- supervisor 自己被杀时，agent 会继续服务；再次运行 exe 会重新接管（命名互斥体保证单实例）。
- supervisor 与 agent 同时被杀时，下一次登录（开机自启）或手动运行 exe 恢复。

## 可选安装形态

`ModeGo-Setup.exe` 是 32 位引导器内嵌 x86/x64 两套 agent，按系统位数解包。双击默认进入
`setup`，复制到 `%LOCALAPPDATA%\ModeAgent`、生成 token、可选自启/托盘（面板固定 300×400）、尝试放行
TCP 47821、启动 supervisor，并打开浏览器引导 GitHub 登录（授权后自动加入账号目录）。

静默部署（GUI 子系统；安装会启动常驻 supervisor，`-Wait` 可能一直等到守护进程退出，
所以用健康检查轮询判断完成）：

```powershell
Start-Process ".\ModeGo-Setup.exe" -ArgumentList "install","--quiet","--no-firewall","--no-autostart"
while (-not (Test-NetConnection 127.0.0.1 -Port 47821 -InformationLevel Quiet)) { Start-Sleep 1 }
mode-agent.exe login
# 或者显式指定安装目录
Start-Process ".\ModeGo-Setup.exe" -ArgumentList "install","--dir","D:\ModeAgent","--port","47821","--quiet"
```

卸载：

```powershell
Start-Process -Wait ".\ModeGo-Setup.exe" -ArgumentList "uninstall","--purge","--yes"
```

交互安装对话框可取消「开机自启」「显示托盘图标」；CLI 对应 `--no-autostart`、
`--no-tray`，反向开启用 `--autostart`、`--tray`。本地面板固定 300×400，不提供关闭选项。

## CLI

Windows exe 是 GUI 子系统；从 cmd/PowerShell 运行 CLI 命令时会附加父控制台，输出仍可见。
stdout 被重定向到文件/管道时保留原句柄，脚本能正常读取。

```powershell
$env:MODE_AGENT_TOKEN = "<token>"

mode-agent.exe remote info --host 192.168.1.20:47821
mode-agent.exe remote exec --host 192.168.1.20:47821 --command "ipconfig /all"
mode-agent.exe remote screen --host 192.168.1.20:47821 --out screen.png
mode-agent.exe remote mouse  --host 192.168.1.20:47821 --action click --x 640 --y 360
mode-agent.exe remote key    --host 192.168.1.20:47821 --text "hello"
mode-agent.exe remote db-query --host 192.168.1.20:47821 --dsn erp --sql "SELECT TOP 10 * FROM orders"
```

手动初始化 / 前台调试：

```powershell
mode-agent.exe init --lan --tls
mode-agent.exe serve --config C:\path\to\mode-agent.json --ui
mode-agent.exe serve --config C:\path\to\mode-agent.json --no-ui
```

TLS + 自签证书：

```powershell
mode-agent.exe remote info --host 192.168.1.20:47821 --tls `
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
  },
  "panel": {
    "enabled": true,
    "showTray": true,
    "autostart": true
  }
}
```

- `allowedRoots` 非空时，所有文件操作与命令 cwd 都被限制在这些根内。
- 写、删、桌面控制默认关闭；在明确需要时逐项打开。
- `panel` 只影响本地 UI（300×400 面板、托盘、登录自启），不是权限边界；面板内容读写仍受
  `allowDesktopControl` 控制。
- 数据库优先使用命名 `dsns`，避免把密码放进模型上下文。
- 审计日志：`dataDir/audit.log`，不记录命令、SQL、令牌或文件内容。

## 接入 Mode（MCP）

主路径由原生系统菜单「配对...」完成。手动配置：设置 → MCP → 添加 HTTP MCP Server：

- URL：`http://<target>:47821/mcp`（TLS 时 `https://...`）
- Header：`Authorization: Bearer <token>`
- 连接后工具名前缀形如 `mcp__<serverName>__remote_exec`

可用工具：

`remote_info`、`remote_exec`、`remote_job_start/status/write/cancel/list`、
`remote_fs_list/read/write/stat/remove`、`remote_process_list/kill`、
`remote_screen_capture`、`remote_mouse`、`remote_keyboard`、
`remote_windows_list`、`remote_window_focus`、`remote_panel_show`、
`remote_panel_clear`、`remote_panel_status`、`remote_db_drivers`、`remote_db_query`。

Mode 的 HTTP MCP 客户端不做自定义证书指纹校验；自签证书请改用受信任证书、反向代理或 VPN，
或使用上面的 CLI（支持 `--tls-fingerprint`）。

## 能力限制

- **无 PTY**：作业是管道语义，适合命令行工具与数据库客户端；全屏 TUI 不保证。
- **Session 0**：作为 Windows 服务运行时无法控制交互桌面；本实现不使用服务形态，默认以登录用户运行。
- **UIPI**：非提权 agent 无法操作高完整性窗口，也无法注入 Ctrl+Alt+Del。
- **ODBC 位数**：32 位 agent 只看到 32 位 ODBC 驱动；x64 机器优先使用 `mode-agent-win-x64.exe`。
- **无 NAT 穿透**：需要目标可达（LAN、VPN、端口映射或反向代理）。
- **单实例**：同一配置只允许一个 supervisor；端口被另一个 token 不同的 agent 占用时会提示并等待。

## 开发与测试

```powershell
# Go 1.10.8（XP 兼容语法与行为）
GO111MODULE=off GOOS=windows GOARCH=386 CGO_ENABLED=0 ../../.tools/go/bin/go.exe test .

# 当前 Go
../../.tools/go-modern/go/bin/go.exe test .

# 真实 MCP 客户端联调（需要先构建 mode-agent-xp-x86.exe）
pnpm exec tsx --test apps/remote-agent/test/mcp-client.test.mts
```

构建脚本会给所有 Windows 产物写入 GUI 子系统与 Mode 图标；`setup/pack` 只负责把引导器与
x86/x64 载荷拼成单个 `ModeGo-Setup.exe`。
