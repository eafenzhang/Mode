# 旧版 Windows 远程被控运行时（mode-agent）

状态：2026-10-20 已实现；Windows 10 x86/x64、真实 MCP 客户端、一键安装/配对与桌面端配对逻辑验证通过，XP SP3 与桌面 UI 点击验收待执行。2026-10-20 追加：远程桌面、远程工作区与 Mode Agent 统一使用桌面 LAN 一次性配对码，远程控制页合并为单一设备列表（[remote-control-unified-pairing.md](./remote-control-unified-pairing.md)）。2026-10-22 追加：交付形态改为**单个免安装 GUI exe**，带守护重启、托盘、固定 300×400 受控面板、无控制台与统一图标；规则见 [mode-agent-tray-panel.md](./mode-agent-tray-panel.md)。关联：[lan-remote-attach.md](./lan-remote-attach.md)（既有 LAN 挂接）、[lan-host-server-mode.md](./lan-host-server-mode.md)（Node 形态被连端）、[computer-use-windows-runtime.md](./computer-use-windows-runtime.md)（桌面控制能力边界）。

## 背景与问题

现有 LAN 远程工作区要求对端运行完整的 Mode Desktop/Host（Node + Tauri）。这条链路无法覆盖
Windows XP，也无法在一台只装了最小运行时的老机器上运行。用户需要一个**可独立部署的 Windows 被控端**：
Mode 通过它执行命令、读取数据库、截屏和注入鼠标键盘，覆盖 XP 到 Win11。

约束：

- XP 没有可用的现代 Node/Tauri；必须产出**单文件、无运行时依赖**的 32 位原生程序。
- XP 的 Schannel 只支持 TLS 1.0，不能依赖系统 TLS 栈；但被控端本身可以是客户端 TLS 的终点。
- 被控端一旦可达，权限等价于运行它的 Windows 用户；安全边界必须先于能力实现。

## 目标与非目标

**做**：

- 一个 `mode-agent` 单文件程序，Windows XP / Vista / 7 / 8 / 8.1 / 10 / 11（x86 与 x64 via WoW64）。
- 本地 HTTP API：命令/作业、文件、进程、屏幕、鼠标键盘、窗口、ODBC 数据库、系统信息。
- 标准 **MCP Streamable HTTP** 端点（`/mcp`），Mode 现有 HTTP MCP 客户端可直接接入并发现工具。
- 跨平台命令行客户端（同一源码的 `remote` 子命令），用于初始化、联调和故障排查。
- 可选 TLS（证书文件 + 客户端 SHA-256 指纹固定）、令牌鉴权、审计日志、能力开关与路径沙箱。

**不做（本期）**：

- 不做 NAT 穿透/中继；要求目标可达（LAN、VPN、端口映射或反向代理）。
- 不做真正的 PTY/TUI。作业 stdin/stdout 基于匿名管道，适合命令行工具，不保证全屏控制台程序。
- 不做 Windows 服务形态的会话切换；Service Session 0 下的桌面能力明确标注不可用。
- 不做多用户切换、提权（UAC）绕过、锁屏解锁、Ctrl+Alt+Del 注入。
- 不做按命令的细粒度策略引擎；危险能力由配置开关 + Mode 侧审批共同约束。
- 不改变现有桌面端 LAN 挂接协议；这是独立的被控端产品面。

## 运行时选型

**Go 1.10.8 负责 XP 产物，当前 Go 负责现代 Windows 产物，同一份源码。**

- Go 1.10 是最后一个官方支持 Windows XP/Vista 的 Go 版本（Go 1.10 release notes）。
- 只用标准库，`CGO_ENABLED=0`，`GOARCH=386`，产出静态单文件，XP 与 x64 Windows 都可运行。
- 现代 Windows 用当前 Go 重新编译，获得安全修复与新 TLS 实现；源码保持 Go 1.10 语法子集。
- 不使用任何第三方 Go 模块，避免 GOPATH/module 与老工具链的依赖解析问题。
- 安全口径：XP 产物使用已 EOL 的标准库，**只应部署在受控 LAN/VPN**；公网暴露必须走 TLS 终止
  代理或现代 Windows 产物。构建脚本会在 XP 产物上打印该警告。

## 架构与所有权

```text
┌──────────── Mode Desktop / CLI ────────────┐
│  MCP HTTP client（现有 @modelcontextprotocol）│
│  或 mode-agent remote CLI（联调/运维）        │
└───────────────┬────────────────────────────┘
                │ HTTP(S) + Bearer token
                │  /mcp（MCP JSON-RPC）
                │  /v1/*（REST，CLI/诊断）
┌───────────────▼────────────────────────────┐
│ mode-agent（目标 Windows 机器）              │
│  - 鉴权 / 审计 / 并发闸门 / 路径沙箱          │
│  - 作业运行时（exec、stdin、输出环、超时）     │
│  - 能力：fs / process / screen / input / db  │
└───────────────┬────────────────────────────┘
                │ Win32 / ODBC 公共 API
        ┌───────▼────────┐
        │ Windows XP–11   │
        └────────────────┘
```

- **唯一状态所有者 = 目标机上的 `mode-agent`**：配置、作业、输出缓冲、审计日志都在被控端。
  Mode 侧不保存作业队列或输出快照；Mode 断开后作业按 TTL 继续或超时终止，由被控端决定。
- **MCP 会话无状态**：`/mcp` 不要求 `Mcp-Session-Id`，每个 POST 独立处理；GET 返回 405
  （客户端按规范视为“不提供 SSE 流”）。通知类请求返回 202。
- **REST 与 MCP 共用同一组能力函数**：不存在第二套执行路径；MCP 只是协议适配层。

## 配置与安全边界

配置文件默认在可执行文件同目录的 `mode-agent.json`；目录不可写时回退到
`%APPDATA%\ModeAgent\mode-agent.json`。`init` 生成 32 字节随机令牌（base64url），只在初始化时
打印一次；令牌可用 `mode-agent token` 在本机读取。

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
  "dsns": {},
  "panel": {
    "enabled": true,
    "showTray": true,
    "autostart": true
  }
}
```

`panel` 只影响本地 UI（300×400 动画面板、托盘、登录自启），不是权限边界；面板内容的读写仍由
`allowDesktopControl` 控制。详见 [mode-agent-tray-panel.md](./mode-agent-tray-panel.md)。

规则：

1. **令牌**：`Authorization: Bearer <token>` 或 `X-Mode-Agent-Token`。常量时间比较；缺失/错误一律 401。
   `/health` 免鉴权但只返回名称、版本、时间，不泄露主机信息；`/v1/pair` 免鉴权但必须持有效
   一次性配对码（8 位、10 分钟 TTL、每 IP 每分钟最多 5 次失败）。
2. **监听**：默认 `127.0.0.1`；`init --lan` 才写 `0.0.0.0`。`allowCidrs` 非空时按来源 IP 做 CIDR 过滤。
3. **能力开关**：`allowShell` 控制命令与进程终止；`allowWrite`/`allowDelete` 控制文件写删；
   `allowDesktopControl` 控制截屏与输入；`allowDb`/`allowDbWrite` 控制数据库。关闭的能力返回
   `capability_disabled`，不是 404。
4. **路径沙箱**：`allowedRoots` 非空时，所有文件操作（含 cwd）必须落在根内；Windows 路径大小写
   不敏感，先 `Clean/Abs`，存在时 `EvalSymlinks`，再用 `filepath.Rel` 判定。默认空 = 不限制，但
   写/删默认关闭。
5. **数据库**：默认只读。只读时同时设置 ODBC `SQL_ATTR_ACCESS_MODE=SQL_MODE_READ_ONLY` 并拒绝
   非 `SELECT/WITH/SHOW/DESCRIBE/EXPLAIN/PRAGMA` 开头的语句。凭据优先放配置里的命名 `dsns`，
   MCP/REST 只传 DSN 名，避免模型上下文反复携带密码。
6. **TLS**：`init --tls` 生成自签证书与指纹；客户端用 SHA-256 指纹固定。Mode 的 HTTP MCP 客户端
   不做自定义指纹校验，因此公网/MCP 场景应使用受信任证书、反向代理或 VPN。
7. **审计**：`dataDir/audit.log` JSONL，记录时间、请求 ID、来源、方法、路径、状态、耗时、错误码；
   **不记录命令内容、SQL、连接串、令牌、文件内容**。单文件 10 MiB 轮转一次。
8. **并发**：exec 最多 4 个并发，桌面能力串行，DB 最多 4 个并发；满载返回 `busy`，不排队堆积。
9. **权限等价**：agent 以启动它的 Windows 用户身份运行。非提权进程无法操作高完整性窗口
   （UIPI），无法注入 Ctrl+Alt+Del；这些是明确的失败语义，不尝试绕过。

## 免安装运行与可选安装

目标：目标机器上只需复制并双击一个 GUI exe，完成令牌生成、可选登录自启动、守护启动；没有
控制台窗口，也不需要安装目录。完整规则见 [mode-agent-tray-panel.md](./mode-agent-tray-panel.md)。

### 免安装形态（推荐）

- 产物：`mode-agent-win-x64.exe` / `mode-agent-win-x86.exe` / `mode-agent-xp-x86.exe`，均为
  GUI 子系统、单文件、无运行时依赖。
- 首次双击：显示首次运行对话框（开机自启 / 托盘 / 面板），在 exe 同目录生成
  `mode-agent.json`；目录不可写时回退 `%APPDATA%\ModeAgent\mode-agent.json`。
- 默认：`bind=0.0.0.0`、`port=47821`、`allowShell=true`、`allowDesktopControl=true`、
  `allowDb=true`；`allowWrite`、`allowDelete`、`allowDbWrite` 保持关闭。
- 启动后：`start` supervisor 拉起 `serve --ui` 子进程，创建托盘与固定 300×400 动画面板；
  HTTP 就绪后自动弹出配对页（只输入 6 位配对码）。之后点击面板标题栏左侧的 Mode 图标，
  菜单「设置...」和「配对...」分别打开两个独立页面，不需要运行安装器。
- 登录自启动：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\ModeAgent`，命令为
  `"<exe>" start --config "<config>"`；不经过 `cmd.exe`，登录时不闪控制台。
- 守护：`serve` 子进程崩溃或被结束后自动重启；托盘退出、`/v1/shutdown` 或 `mode-agent stop`
  写 `mode-agent.stop`，supervisor 看到后不再重启并退出。
- 日志：`agent.log`、`supervisor.log`，与配置同目录；面板内容不落盘。
- 移动 exe 后自启项会失效；重新运行一次 exe 并在托盘中重开自启即可。

### 可选安装形态

- **`ModeAgent-Setup.exe`**：32 位引导器内嵌 x86/x64 两套 agent，按系统位数解包；双击默认进入
  `setup`（GUI 子系统，无控制台）。
- 安装目录：`%LOCALAPPDATA%\ModeAgent`（Vista+）或 `%APPDATA%\ModeAgent`（XP）。
  `install --dir <path>` 可覆盖。
- 布局：`<installDir>\mode-agent.exe`、`mode-agent.json`、`install-info.txt`、`agent.pid`；
  日志与 stop 文件也在这里。
- 安装默认值同免安装形态；交互安装对话框可取消自启/托盘（本地面板固定 300×400）。
  `--no-autostart`、`--no-tray` 为脚本路径；`--autostart`、`--tray` 可重新开启。
- 安装后启动 supervisor（不是直接 `serve`），轮询 `/health` 最多 10 秒；`--no-start` 只安装。
- `install`/`setup --yes` 不做交互确认；`--quiet` 不弹配对框，调用方负责 `enroll` 或手动 MCP。
- 防火墙规则 best-effort：`--no-firewall` 跳过；规则名固定 `ModeAgent`，端口取实际配置端口。
- 安装并启动后会弹配对框；输入桌面端生成的 6 位配对码后向
  `POST /api/remote-agent/enroll` 登记。取消则保留安装，`install-info.txt` 记录
  `enrollment=canceled`，可稍后 `mode-agent enroll`。
- `uninstall`：写 stop 文件、停止 agent、移除自启、尝试删除防火墙规则。
  `uninstall --purge` 额外删除配置、证书/私钥、连接信息、审计日志、PID、日志与安装目录。
- Windows 无法删除运行中的自身 exe；从安装包运行卸载时可以删除已安装的副本。

### 单实例与升级

- supervisor 使用命名互斥体 `Local\ModeAgentSupervisor_<config-path-hash>`；再次运行 exe
  只打开面板，不启动第二套服务。
- `serve` 启动时把 PID 写入 `agent.pid`，退出时删除；安装/卸载先写 stop 文件再按 PID 收口。
- 端口被另一个 token 不同的 agent 占用时，supervisor 记录日志并弹一次警告，每 10 秒重试；
  不会把对方当成自己的 agent。
- 同一端口重复启动由 `net.Listen` 失败兜底；不存在第二套业务写入路径。

### 构建与图标

- 所有 Windows 产物用 `-H windowsgui` 构建；CLI 命令在 stdout 未被重定向时附加父控制台。
- 图标由 `set-icon.ps1` 直接写入 PE 的 `RT_GROUP_ICON`（组 ID 1）与多尺寸 `RT_ICON`，
  不依赖 rcedit；exe、窗口、托盘、首次运行/配对对话框统一使用 `assets/mode-agent.ico`。
- 安装包 = 引导器 exe + zip 载荷 + footer；引导器与 agent 都带图标，且都是 GUI 子系统。
  引导器默认参数为 `setup`，双击安装包不会误入免安装模式。

## API 面

主配对路径不在 agent 上：桌面端 LAN 监听暴露 `POST /api/remote-agent/enroll`（配对码自证，
不要求既有令牌）；agent 的 `/v1/pair`/`/v1/pair-code` 是备用方向。agent 的 REST 请求/响应为
UTF-8 JSON；错误统一：

```json
{ "ok": false, "error": { "code": "capability_disabled", "message": "..." }, "requestId": "..." }
```

成功为 `{ "ok": true, "result": { ... }, "requestId": "..." }`。请求体默认上限 1 MiB，文件写入
上限 16 MiB；响应上限 16 MiB。

| 方法 | 路径 | 语义 |
| --- | --- | --- |
| GET | `/health` | 免鉴权健康检查 |
| POST | `/v1/pair` | 备用方向：免鉴权（配对码即凭据），配对码换长期 token；仅该端点开放 CORS |
| POST | `/v1/pair-code` | 备用方向：令牌鉴权，生成/轮换 8 位配对码（10 分钟 TTL） |
| POST | `/v1/shutdown` | 令牌鉴权：优雅停止 agent（写 stop 文件，supervisor 不再重启） |
| GET | `/v1/info` | 主机、Windows 版本、会话、能力开关、屏幕尺寸 |
| GET | `/v1/panel` | 令牌鉴权 + `allowDesktopControl`：读取面板状态 |
| POST | `/v1/panel` | 令牌鉴权 + `allowDesktopControl`：写入标题/文本/图片并显示面板 |
| DELETE | `/v1/panel` | 令牌鉴权 + `allowDesktopControl`：清空面板 |
| POST | `/v1/exec` | 同步执行，返回 exitCode/stdout/stderr/duration/truncated |
| POST | `/v1/jobs` | 启动作业，立即返回 jobId |
| GET | `/v1/jobs` | 列出作业摘要 |
| GET | `/v1/jobs/{id}?stdoutOffset=&stderrOffset=` | 增量读取输出与状态 |
| POST | `/v1/jobs/{id}/stdin` | 写入 stdin |
| POST | `/v1/jobs/{id}/cancel` | 终止进程树 |
| POST | `/v1/fs/list` | 列目录 |
| POST | `/v1/fs/read` | 读文件（文本或 base64） |
| POST | `/v1/fs/write` | 写文件（原子替换；可选追加） |
| POST | `/v1/fs/stat` | 文件元信息 |
| POST | `/v1/fs/remove` | 删除文件/目录（需 allowDelete） |
| GET | `/v1/processes` | 进程列表 |
| POST | `/v1/process/kill` | 结束进程 |
| POST | `/v1/screen/capture` | 截屏，PNG base64，可按 maxWidth/maxHeight 缩放 |
| POST | `/v1/input/mouse` | 移动/点击/按下/抬起/滚轮/拖拽 |
| POST | `/v1/input/key` | 文本输入或按键组合 |
| GET | `/v1/windows` | 顶层窗口列表（标题、PID、句柄、矩形） |
| POST | `/v1/window/focus` | 置前窗口 |
| GET | `/v1/db/drivers` | 枚举 ODBC 驱动 |
| POST | `/v1/db/query` | 执行 SQL，返回列与行（有行数/单元格上限） |
| POST | `/mcp` | MCP Streamable HTTP；`initialize`/`tools/list`/`tools/call`/`ping` |
| GET/DELETE | `/mcp` | 405（不支持独立 SSE 流/显式会话终止） |

### 作业语义

- 作业由 `POST /v1/jobs` 创建；`exec` 是“创建 + 等待”的语法糖。
- stdout/stderr 各自维护**绝对偏移的环形缓冲**，默认 1 MiB；超出后丢弃最旧数据并标记
  `truncated:true`。增量读取从调用方给出的 offset 开始，返回 `nextOffset`。
- 超时/取消通过 Job Object 终止整棵进程树；`status` 为 `exited|failed|timed_out|canceled`。
- 作业在结束 10 分钟后回收；回收后查询返回 `job_not_found`。
- `shell`：`cmd`（默认，`cmd.exe /D /S /C`）、`powershell`（`-NoProfile -NonInteractive -Command`）、
  `none`（直接启动 command + args）。`cmd/powershell` 下 `args` 不参与拼接，避免二次转义。
- 输出编码：默认自动；非法 UTF-8 时按控制台 OEM 代码页转 UTF-8（`MultiByteToWideChar`），
  可用 `encoding` 强制 `utf8|oem|ansi`。

### 屏幕与输入

- 截屏使用 GDI（`GetDC(0)` + `BitBlt` + `GetDIBits`），覆盖虚拟桌面多显示器；输出 PNG。
- 默认最大宽 1600；超出按最近邻缩放，避免 MCP 结果过大。
- 鼠标坐标使用虚拟桌面绝对坐标（与截图像素一致）；`click` 是 down+up 的原子操作。
- 键盘支持 `--text`（Unicode 注入）与 `--key "CTRL+S"`（VK 码组合）。不支持安全注意序列。
- 当前进程不在活动控制台会话时，`desktopAvailable=false`，桌面能力返回
  `desktop_unavailable`（Service Session 0 的预期行为）。

### 数据库

- `POST /v1/db/query` 请求：`{ "dsn": "erp", "connection": "...", "sql": "...", "maxRows": 200,
  "timeoutMs": 30000, "readOnly": true }`。`dsn` 与 `connection` 二选一。
- 返回 `{ columns: [{name,type}], rows: [[string|null]], rowCount, truncated }`。
- 单单元格上限 64 KiB，超出标记截断；BLOB 以 base64 返回并标注 `binary`。
- `maxRows` 上限 5000；查询超时由 ODBC `SQL_ATTR_QUERY_TIMEOUT` 控制。

### MCP 工具

服务名 `mode-agent`，工具均为 `remote_*`：

`remote_info`、`remote_exec`、`remote_job_start`、`remote_job_status`、`remote_job_write`、
`remote_job_cancel`、`remote_job_list`、`remote_fs_list`、`remote_fs_read`、`remote_fs_write`、
`remote_fs_stat`、`remote_fs_remove`、`remote_process_list`、`remote_process_kill`、
`remote_screen_capture`、`remote_mouse`、`remote_keyboard`、`remote_windows_list`、
`remote_window_focus`、`remote_panel_show`、`remote_panel_clear`、`remote_panel_status`、
`remote_db_drivers`、`remote_db_query`。

- `tools/list` 返回 JSON Schema；`tools/call` 返回 MCP content blocks。
- 截屏返回 `{type:"image", mimeType:"image/png", data:<base64>}`，另附尺寸文本块。
- 错误以 `isError:true` 的文本块返回，同时保留机器可读错误码前缀（`code: message`）。
- 工具描述明确“这是远程 Windows 机器、能力受配置开关限制、破坏性操作需用户明确意图”。

## 桌面端五步闭环

1. **桌面端生成配对码**：远程控制页 →「生成配对码」。桌面确保局域网访问监听已开启，生成
   6 位一次性配对码（5 分钟 TTL），并展示
   `mode-agent://enroll?host=<桌面地址>&port=<监听端口>&code=<配对码>`。这个码与远程工作区 /
   远程桌面接入共用同一个 `lanAccessServer.pairCode`；不再另生成第二套码。
2. **目标机运行并输入配对码**：直接把免安装 exe 复制到目标机双击（首次运行对话框 → 生成配置）；
   或运行可选的 `ModeAgent-Setup.exe`。免安装 exe 启动后会自动弹出配对页；之后可从
   点击面板标题栏左侧的 Mode 图标打开原生系统菜单，选择「设置...」或「配对...」。
   被控端向 `POST http://<桌面>:<端口>/api/remote-agent/enroll` 发送自己的
   agentId、地址、端口、token、版本与平台。
3. **桌面端自动安装 MCP**：桌面验证配对码、探测被控端 `/health`，成功后写入一个用户级 HTTP
   MCP server（URL `http://<目标>:<端口>/mcp`，Header `Authorization: Bearer <token>`）。
4. **列表**：远程控制页把 LAN 已配对设备与 Mode Agent 设备合并到同一列表，展示名称、地址、
   连接状态与能力/工具数；MCP 配置是 Mode Agent 设备的唯一事实源，LAN 配对记录是 LAN 设备的
   唯一事实源，不另存一份设备表（[remote-control-unified-pairing.md](./remote-control-unified-pairing.md)）。
5. **发起对话**：确保该 MCP server 启用，先切到对话主视图，再创建新任务草稿并预填远程指令
   （要求使用 `mcp__<serverName>__remote_*`，不要用本机工具）；用户补完任务后发送。

备用方向只保留 CLI：被控端 `mode-agent pair` 生成 8 位目标端配对码，桌面端不再提供「手动配对」
表单，也不再展示这套 8 位码；需要该方向时用 CLI / 手动 MCP 配置。手动 MCP 路径保留：设置 →
MCP → 添加 HTTP MCP Server，或在 Mode 的 Bash 工具中调用 `mode-agent remote exec ...`。

### 配对失败语义

| 场景 | 结果 |
| --- | --- |
| 配对码错误/过期/已消费 | `invalid_pair_code` 401，桌面提示重新生成 |
| 同一分钟失败超过 20 次 | `too_many_attempts` 401 |
| 被控端 `/health` 不可达 | `agent_unreachable` 401 |
| MCP 配置写入失败 | 配对失败并保留错误；不展示“已配对” |
| MCP server 离线 | 列表显示离线；仍可发起对话，由运行时重连 |

### 安全口径

- 安装器默认开启命令与桌面控制（这是产品用途），但必须先弹窗确认；写/删仍默认关闭。
- 桌面端配对码复用现有 LAN 一次性码：6 位、5 分钟 TTL、单次消费；反向登记端点额外限制
  每分钟最多 20 次失败，并在写 MCP 前探测被控端 `/health`。
- 被控端把长期 token 发给桌面端，桌面端存入 MCP 配置的 Authorization header；链路只应在
  受控 LAN/VPN 上使用。桌面端「移除被控端」只删本机配置，被控端 token 仍有效；需要撤销时
  在目标机执行 `install --force` 轮换。
- 备用目标端配对端点 `/v1/pair` 仅对桌面 renderer 需要 CORS；主路径是目标端主动 POST 桌面。

## 验收场景

1. `go test ./...`（Go 1.10.8 与当前 Go 各跑一遍）：协议编解码、路径沙箱、输出环、只读 SQL 判定。
2. `mode-agent init --lan --tls` 生成配置/证书；启动 `serve`；`/health` 200；无令牌的 `/v1/info` 401。
3. 带令牌 `GET /v1/info` 返回 Windows 版本、能力开关、屏幕尺寸。
4. `POST /v1/exec` 执行 `ver`（XP/10）返回 exitCode 0；中文输出在 CP936 机器上不乱码。
5. `POST /v1/jobs` 启动长命令，增量读取输出，`stdin` 写入可见，`cancel` 后进程树消失。
6. 文件：在 `allowedRoots` 外读/写被拒绝；写关闭时 `capability_disabled`；读二进制返回 base64。
7. 截屏返回 PNG，尺寸与虚拟桌面一致（缩放后符合 maxWidth）；关闭 `allowDesktopControl` 时拒绝。
8. 鼠标/键盘：在测试窗口内完成点击与文本输入；Service Session 0 下返回 `desktop_unavailable`。
9. ODBC：`/v1/db/drivers` 列出驱动；只读连接执行 `SELECT` 成功，`UPDATE` 被只读策略拒绝。
10. MCP：用 `@modelcontextprotocol/client` 的 `StreamableHTTPClientTransport` 连接 `/mcp`，
    完成 initialize → tools/list → remote_info → remote_exec → remote_screen_capture。
11. 免安装/守护：全新目录双击 GUI exe → 首次运行对话框 → 生成配置、无控制台、托盘与
    300×400 动画面板；结束 `serve` 子进程后数秒内自动重启；`stop` 后 supervisor 与 agent 都退出。
12. 可选安装/反向配对：单文件 `ModeAgent-Setup.exe` 在 x64 上解包 amd64、在模拟 32 位环境
    解包 386；双击默认进入 `setup`；安装后弹窗输入桌面端 6 位配对码；`POST /api/remote-agent/enroll`
    写 MCP、二次同码 401、不可达 agent 401；`uninstall --purge` 停止进程并删除安装目录。
13. 面板/图标：`remote_panel_show` 渲染标题+文本+PNG，`remote_panel_clear` 恢复默认代码雨动画；
    exe/窗口/托盘/对话框均显示 Mode 图标；`build.ps1` 四个 Windows 产物子系统=2。
14. 桌面闭环：远程控制页生成 6 位配对码 → 被控端输入 → 同一列表出现 `mode-agent-*`
    设备（与 LAN 设备同屏）→「发起对话」切到对话视图并创建预填远程指令的新任务草稿。
15. 审计日志包含请求记录且不包含命令/SQL/令牌明文；10 MiB 轮转。
16. 构建：Go 1.10.8 产出 `mode-agent-xp-x86.exe`；现代 Go 产出 `mode-agent-win-x64.exe`；
    单文件安装器内嵌两套 agent 并带 Mode 图标。实机 XP 验收需一台 XP SP3 机器（当前环境不可用，
    登记为待办）。

## 失败语义

| 场景 | code | HTTP |
| --- | --- | --- |
| 缺/错令牌 | `unauthorized` | 401 |
| CIDR 不允许 | `forbidden` | 403 |
| 能力开关关闭 | `capability_disabled` | 403 |
| 并发满载 | `busy` | 429 |
| 参数非法 | `invalid_argument` | 400 |
| 路径越界 | `path_not_allowed` | 403 |
| 文件不存在 | `not_found` | 404 |
| 作业不存在/已回收 | `job_not_found` | 404 |
| 命令启动失败 | `exec_failed` | 400 |
| 非活动会话桌面能力 | `desktop_unavailable` | 409 |
| 数据库未配置/驱动缺失 | `db_unavailable` | 400 |
| 内部错误 | `internal_error` | 500 |

## 状态与事件顺序

```text
init: 生成 config/token/cert → 写盘 → 打印路径与指纹
serve: 读 config → 打开审计 → 监听 → 每个请求: 鉴权 → CIDR → 能力闸门 → 执行 → 审计
exec: 创建 Job → 启动进程 → 分配 Job Object → 读线程填环 → Wait → 状态终结 → TTL 回收
MCP: POST /mcp → 解析 JSON-RPC → tools/call → 同一能力函数 → content blocks → 200 JSON
```

## clean-room 输入

- 本仓 specs：本文件、`lan-remote-attach.md`、`computer-use-windows-runtime.md`。
- 公开协议：Model Context Protocol Streamable HTTP 规范与 `@modelcontextprotocol/client` 的
  `StreamableHTTPClientTransport` 行为（仅按协议行为对接）。
- 平台 API：Microsoft Win32/GDI/ODBC 公开文档。
- 不读取上游 `zai-org/ZCode` 或本仓上游衍生实现。

## 待办（按优先级）

1. 实机 XP SP3 验收（当前机器为 Windows 10，只能验证 32 位产物可运行）。
2. 服务形态（SCM）+ 会话内 helper（解决 Service Session 0 无法控制桌面的问题）。
3. 中继/NAT 穿透；本期只支持直接可达。
4. 命令级策略（允许/拒绝规则）与危险操作二次确认；本期依赖 Mode 审批 + 能力开关。
