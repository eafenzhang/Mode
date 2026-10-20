# 旧版 Windows 远程被控运行时（mode-agent）实施计划

状态：2026-10-20 已完成 Windows 10 x86/x64 与 MCP 客户端验收；XP SP3 实机待验收。规格：[legacy-windows-remote-agent.md](../specs/legacy-windows-remote-agent.md)。

## 为什么现在做

现有 LAN 远程工作区依赖完整 Mode Host（Node），无法覆盖 XP。需要一台老机器上可独立部署的
被控端，Mode 通过标准 MCP HTTP 或 CLI 执行命令、读库、截屏与注入输入。

## 里程碑

1. **规格与安全边界**（完成）：平台选型、配置、令牌、能力开关、路径沙箱、审计、失败语义。
2. **被控端骨架**（完成）：配置/初始化/自签证书、HTTP 服务、鉴权、审计、信息面。
3. **能力面**（完成）：exec/jobs、fs、process、screen、input、window、ODBC。
4. **MCP 端点**（完成）：stateless Streamable HTTP，`initialize`/`tools/list`/`tools/call`/`ping`。
5. **客户端**（完成）：跨平台 `remote` 子命令，用于联调、验收与应急。
6. **构建**（完成）：Go 1.10.8 产 XP x86；当前 Go 产 Win x64/x86 与各平台客户端。
7. **验证**（完成）：Go 单测、真实 HTTP/MCP 联调、Mode 适配器兼容、typecheck/lint/架构检查。

## 文件布局

```text
apps/remote-agent/
├── main.go / config.go / protocol.go
├── server.go / mcp.go / audit.go / ring.go / fs.go / exec.go
├── client.go
├── desktop_windows.go / db_windows.go / winapi_windows.go
├── serve_windows.go / serve_stub.go
├── proc_windows.go / proc_stub.go / job_stub.go
├── autostart_windows.go / autostart_stub.go / encoding_stub.go / platform_stub.go
├── *_test.go
├── test/mcp-client.test.mts  # 真实 MCP 客户端联调
├── build.ps1 / build.sh
└── README.md
```

## 关键取舍

- **Go 1.10.8 + 当前 Go 双构建**：Go 1.10 是最后一个官方支持 XP 的版本；源码保持 1.10 语法子集。
- **x64 产物**：x64 Windows 优先用 64 位产物，才能看到 64 位 ODBC 驱动；XP 用 x86。
- **MCP 无状态**：不实现 `Mcp-Session-Id` 与独立 SSE GET；GET 返回 405，符合客户端预期。
- **不做 PTY**：作业是管道语义；全屏 TUI 不保证。先覆盖命令行工具与数据库客户端。
- **不默认开写/桌面控制**：`allowWrite`/`allowDelete`/`allowDesktopControl` 默认关闭，显式开启。

## 一键安装（追加里程碑）

- [x] 无参数双击进入 Windows 确认框；确认后复制自身、生成配置/令牌、写自启动、尝试防火墙、
      启动 agent、输出 MCP 地址与令牌。
- [x] `install` / `setup --yes` / `uninstall` / `uninstall --purge` 命令行路径。
- [x] 保留已有 token 的升级路径；`--force` 才轮换。
- [x] PID 文件 + `taskkill` 停止旧进程；`--no-firewall` / `--no-autostart` / `--no-start` /
      `--dir` 用于受控部署与测试。
- [x] 构建产出 `ModeAgent-Setup-x86.exe` 与 `ModeAgent-Setup-x64.exe`。

## 桌面端五步闭环（追加里程碑）

- [x] 桌面端生成 6 位一次性配对码（复用 LAN 监听，5 分钟 TTL）与 `mode-agent://enroll?...` 配对串。
- [x] 单文件 `ModeAgent-Setup.exe`（32 位引导器内嵌 x86/x64 agent），安装后弹窗输入桌面配对串。
- [x] 被控端反向 POST `/api/remote-agent/enroll`：桌面验证码、探测 `/health`、写 HTTP MCP。
- [x] 列表从 MCP store 读取 `mode-agent-*` server，展示连接状态/工具数/移除；支持手动刷新。
- [x] 「发起对话」启用该 MCP 并创建预填远程指令的新任务草稿。
- [x] 桌面/目标都带 Mode 图标（rcedit 写入 exe；单文件安装器继承引导器图标）。
- [x] 旧方向保留：目标端 `pair` 生成 8 位码，桌面「手动配对」调用 `/v1/pair`。
- [ ] 运行中桌面应用的端到端点击验收（生成码 → 目标输入 → 列表 → 对话）待实机执行。

## 验证清单

- [x] Go 1.10.8 与 Go 1.27.3 的 `go test .` 全绿（协议、路径沙箱、输出环、只读 SQL、配对、HTTP/MCP）。
- [x] 真实 `@modelcontextprotocol/client` 完成 initialize → tools/list → remote_info / remote_exec /
      job start-status-cancel / screen capture；x86 与 x64 均通过。
- [x] 单文件 `ModeAgent-Setup.exe`：x64 解包 amd64、模拟 32 位环境解包 386；安装后
      `enrollment=pending`；目标 CLI `enroll` 对 mock 桌面登记成功。
- [x] 桌面反向登记端点单测：写 MCP、一次性配对码、不可达 agent 拒绝。
- [x] 原生配对输入框单测：真实 `DialogBoxIndirectParamW` 创建、填值、确认并读回。
- [x] 单文件安装器/agent 带 Mode 图标（rcedit 写入；从 exe 提取为 Mode logo）。
- [x] UI 测试 442/442；UI 包 typecheck 通过；新文件 lint 0 warning/0 error；架构检查通过。
- [ ] 实机 XP SP3（待用户提供机器）。
- [ ] 运行中桌面应用的完整点击验收（生成码 → 目标输入 → 列表 → 对话）仍待执行。

## 2026-10-20 验证记录

- 构建：`build.ps1` 产出单文件 `ModeAgent-Setup.exe`（7.4 MB，x86+x64 载荷、Mode 图标）、
  x86/x64 agent 与四个客户端；`build.sh` 使用同一打包器。
- 配对：桌面端 `POST /api/remote-agent/enroll` 实测写用户级 HTTP MCP；二次同码 401；不可达
  agent 401；目标 `enroll` 对 mock 桌面发送 code/agentId/host/port/token，返回成功。
- 安装：单文件安装器 `install --quiet` 后 `install-info.txt` 为 `enrollment=pending`；x64
  载荷 `arch=amd64`，模拟 32 位载荷 `arch=386`；`uninstall --purge` 清理进程与目录。
- 门禁：根 `pnpm typecheck`、UI 全量 442 测试、`pnpm architecture:check --changed` 均通过；
  本任务新增文件 lint 0/0。全量 `pnpm lint` 仍有 1 个与本任务无关的 error，位于既有
  `packages/desktop/tests/bundled-agent-runtime.test.mts`（Windows 路径字符串的
  `no-useless-escape`），未擅自修改。

## 未覆盖

- XP SP3 实机（包括 5.1 版本识别、GDI/ODBC 32 位行为、`WTSGetActiveConsoleSessionId` 缺失路径）。
- x64 ODBC 查询（本机没有 64 位 DSN 可测）。
- 键盘注入到真实受控窗口（当前只做了解析与 SendInput 记录布局单测）。
- Service Session 0、UAC/UIPI 失败语义、NAT/中继。
