# Mode Agent 免安装托盘面板

状态：2026-10-22 实现并验证。适用 `apps/remote-agent` 的 Windows 形态（XP–Win11）。
关联：[legacy-windows-remote-agent.md](./legacy-windows-remote-agent.md)（被控端协议与安全边界）、
[remote-control-unified-pairing.md](./remote-control-unified-pairing.md)（配对与设备列表）。

## 背景与问题

原 `ModeAgent-Setup.exe` 安装后是一个无界面的控制台进程：登录自启动固定开启，运行后没有托盘，
用户无法确认它是否在运行，也没有一个稳定的窗口用于显示桌面端推送的信息。用户要求：

- 程序打包成**一个免安装的 exe**，双击即可运行；
- 支持开机自启；
- 程序崩溃或被结束后能自动重启；
- 启动时**不出现命令行窗口**；
- 有应用图标；
- 打开时显示一个固定 `300 × 400` 的主页面：无内容时播放 Matrix 风格快速代码雨，
  内容由 Mode 桌面端控制；
- 可设置是否显示托盘图标。

`ModeAgent-Setup.exe` 保留为可选的“安装到 `%LOCALAPPDATA%` + 防火墙规则 + 卸载”路径，
不再是推荐入口。

## 目标与非目标

**做**：

- 免安装 exe：配置优先落在 exe 同目录；目录不可写时回退 `%APPDATA%\ModeAgent`。
- GUI 子系统构建：双击不创建控制台；CLI 命令按需附加到父控制台。
- 单一守护入口 `start`：当前进程是 supervisor，负责拉起并看护 `serve` 子进程。
- 子进程崩溃/被杀后自动重启；用户主动退出通过 stop 文件区分。
- 安装器/托盘/配置三者都能设置开机自启与托盘图标。
- 固定 `300 × 400` 客户区的面板；无内容时播放默认代码雨动画，内容只能由已配对的桌面端
  通过 MCP/HTTP 写入。
- exe、窗口、托盘、对话框统一使用 Mode 图标。

**不做**：

- 不做 Windows 服务形态；supervisor 与 agent 都在登录用户会话内运行，桌面控制能力不变。
- 不做“supervisor 和 agent 同时被杀仍能立刻自愈”；这种情况由开机自启或手动再启动恢复。
- 不做本地编辑器、浏览器或任意 HTML/URL 渲染；面板只渲染纯文本与 PNG/JPEG 图片。
- 不新增第二套配对或鉴权；沿用被控端 bearer token 与 `allowDesktopControl` 能力闸门。
- 不改变 LAN 协议、MCP 协议或桌面端设备模型。

## 进程与所有权

```text
ModeAgent.exe（GUI 子系统，无控制台）
  └─ start --config ...                 supervisor（命名互斥体，单实例）
       └─ serve --config ... --ui      agent 子进程
            ├─ HTTP/MCP 服务           唯一业务状态所有者
            ├─ 托盘图标 + 固定 300×400 面板
            └─ /v1/panel 内容           仅内存，重启后回到默认动画
```

| 状态 | 唯一所有者 | 说明 |
| --- | --- | --- |
| 配置（panel/autostart/token…） | 配置文件 `mode-agent.json` | GUI、托盘、安装器共同读写 |
| 开机自启 | `HKCU\...\Run\ModeAgent` | 由配置派生；命令固定为 `"exe" start --config "..."` |
| 守护与重启 | `start` supervisor | 健康 + token 探测；单实例命名互斥体 |
| agent 进程 | `serve` 子进程 | HTTP/MCP、审计、作业、面板窗口 |
| 面板内容 | agent 进程内存 | 桌面端写入；不落盘；清空后回到默认动画 |
| 主动退出 | `mode-agent.stop` | 存在即表示用户主动退出，supervisor 不再重启 |

## 产品规则

1. **免安装启动**：双击 exe 且无配置时，显示首次运行对话框（开机自启/托盘），在
   `resolveConfigPath` 选定的路径生成配置，然后以 supervisor 启动；面板固定 300×400。
2. **首次配对页**：新配置首次拉起 `serve` 时带 `--pair-on-start`；HTTP 监听就绪后自动弹出
   配对页，只输入桌面端显示的 **6 位配对码**；被控端通过 LAN UDP 45879 发现 + Tailscale
   单播自动定位桌面端。取消不影响 agent 运行；崩溃重启不会重复弹出。之后随时可从
   原生系统菜单「配对...」重新打开。
3. **配置位置**：优先 exe 同目录 `mode-agent.json`；目录不可写时回退
   `%APPDATA%\ModeAgent\mode-agent.json`。数据目录同目录，日志为 `agent.log`、
   `supervisor.log`；主动退出标记为 `mode-agent.stop`。
4. **窗口尺寸**：面板客户区固定 `300 × 400` 逻辑像素，按系统 DPI 缩放；不可调整大小、
   不可最大化。窗口标题固定 `Mode Agent`。
5. **默认首页动画**：面板无标题/文本/图片时显示 Matrix 风格快速代码雨（黑色背景、绿色字符、
   快速下落，无中央图形与扫描线）；有内容时切换为内容视图，清空后动画恢复。双击 exe、
   双击托盘图标或桌面端写入内容时显示；关闭按钮只隐藏，不改变 agent 运行状态。
6. **内容所有权**：面板内容只存在于 agent 进程内存；唯一写入方是携带有效 token 且
   `allowDesktopControl=true` 的 Mode 桌面端（MCP 或 `/v1/panel`）。
7. **托盘与菜单入口**：`panel.showTray=true` 时创建托盘图标；双击托盘图标只打开面板，
   不提供右键菜单，单击不触发。唯一菜单入口是 **Windows 原生系统菜单**（点击标题栏左侧
   Mode 图标或 Alt+Space）：保留还原/移动/大小/最小化/最大化/关闭，末尾追加
   「设置... / 配对... / 退出 Mode Agent」。关闭托盘只删除图标，不退出进程；再次运行 exe
   或双击托盘仍可打开面板。
8. **设置页与配对页（两个独立入口）**：
   - 设置页/配对页均为 **WS_EX_TOPMOST 置顶原生对话框**，白底、统一现代字体和表头层级，
     不会压在面板或其它应用后面。
   - **设置页**只包含两个开关（自启/托盘），保存后立即生效；本地面板固定 300×400、
     **始终启用**，设置页不显示面板开关。
     - 关闭窗口只隐藏 300×400 窗口；双击托盘图标或重新运行 exe 可再次打开；
     - 关闭托盘移除图标；面板仍可通过运行 exe 打开；
     - 自启开关同步 Run 项；只有用户把本实例自启从开切到关时才删除，避免误删其他实例。
     - supervisor 始终以 `serve --ui` 拉起 agent，保证面板/托盘入口存在；纯后台只能用
       手工 `serve --no-ui`（高级 CLI）启动。
   - **配对页**只接受 6 位配对码：先向 LAN 广播/定向广播与 Tailscale 100.x 单播探测
     `mode-lan-discover-v1`，再从应答里逐个尝试 `/api/remote-agent/enroll`；配对码由桌面端
     校验，5 分钟有效、一次性。完整 `mode-agent://enroll?...` 配对串仍保留给 CLI
     （`enroll --uri`）和手动场景。
   - 安全口径：配对码流程会把 enroll 载荷（含被控端 token）发给发现的 Mode 桌面端；目标
     桌面端用配对码授权。该便利路径只应在可信 LAN/VPN 使用；需要只联系指定桌面时用完整配对串。
9. **开机自启**：配置 `panel.autostart` 是事实源，Run 项是派生入口。Run 项直接执行 GUI
   exe，不经过 `cmd.exe`，因此登录时不闪命令行窗口。便携实例首次运行未勾选时不会删除
   其他实例已有的 Run 项。
10. **守护重启**：supervisor 每 2 秒探测 `/health` 并用 token 请求 `/v1/info`；未就绪则拉起
    `serve`，启动后 10 秒内未健康按 1s/2s/…/30s 退避重试。子进程被 Task Manager 结束或崩溃
    后自动重启；supervisor 自身被杀时 agent 继续运行，下次执行 exe 会重新接管。
11. **主动退出**：托盘「退出」、`/v1/shutdown`、`mode-agent stop` 都会写 stop 文件；agent
    正常退出后 supervisor 看到文件即退出，不触发重启。安装/卸载/覆盖 exe 前先写 stop 文件。
12. **无控制台**：Windows 产物用 `-H windowsgui` 构建；不创建控制台窗口。`remote`、
    `version`、`init`、`install --quiet` 等 CLI 命令只在 stdout 未被重定向时附加父控制台；
    重定向到文件/管道时保留原句柄，脚本仍能拿到输出。
13. **图标**：构建脚本用 `set-icon.ps1` 写入 `RT_GROUP_ICON`（组 ID 1）与多尺寸
    `RT_ICON`；exe、窗口、托盘、首次运行/设置/配对对话框统一加载该图标。不依赖 rcedit。
14. **可选安装形态**：`ModeAgent-Setup.exe` 仍是 GUI 子系统；双击默认进入 `setup`
    （复制到 `%LOCALAPPDATA%`、可选自启/托盘、防火墙、卸载）。安装器的更新页与
    免安装 exe 的「设置...」等价；配对页独立。CLI 自动化不要对 `install` 使用 `Start-Process -Wait`
    （会一直等到常驻 supervisor 退出），应轮询 `/health` 判断完成；`uninstall` 可用 `-Wait`。
15. **兼容性**：Go 1.10 产物（XP）与当前 Go 产物（x64/x86）共用同一实现；除 Win32
    API 外不引入新依赖。非 Windows 构建返回 `capability_unavailable`。

## 内容模型

```json
{
  "title": "可选标题（≤ 200 字符）",
  "text": "可选纯文本（≤ 64 KiB）",
  "imageBase64": "可选 PNG/JPEG，解码后 ≤ 4 MiB",
  "imageMimeType": "image/png | image/jpeg（可省略，按内容探测）"
}
```

- `title`、`text`、`imageBase64` 至少一个非空；空请求返回 `invalid_argument`。
- 文本按 UTF-8 处理，换行保留；面板使用 `DrawTextW` 自动换行，不解析 Markdown/HTML。
- 图片等比缩放适配面板；与文本同时存在时，图片占上半部分，文本在下半部分。
- `remote_panel_status` 不返回图片数据，只返回 `hasImage`、标题、文本长度、更新时间和可见状态。

## 接口

| 方法 | 路径 | 语义 |
| --- | --- | --- |
| GET | `/v1/panel` | 读取面板状态（不含图片内容） |
| POST | `/v1/panel` | 写入标题/文本/图片并显示面板 |
| DELETE | `/v1/panel` | 清空内容（恢复默认代码雨动画，不隐藏窗口） |

MCP 工具（均需 `allowDesktopControl`）：

- `remote_panel_show`：参数同上，写入并显示；
- `remote_panel_clear`：清空；
- `remote_panel_status`：读取状态。

`/v1/panel` 与 MCP 共用同一组内容函数，不存在第二套写入路径。

## 状态与事件顺序

```text
首次运行: 无配置 → 首次运行对话框 → 写 config(panel.*) →（可选）写 Run 项
          → start supervisor → 拉起 serve --ui --pair-on-start → 托盘 + 面板
          → HTTP 就绪 → 自动弹出配对页
双击 exe: 读取配置 → 已有 supervisor 则发消息打开面板
          → 否则 start supervisor（--open-panel）
设置页: 原生系统菜单「设置...」→ 修改 config → 立即应用自启/托盘
配对页: 原生系统菜单「配对...」→ 输入 6 位码 → LAN/Tailscale 发现
          → 逐个 POST /api/remote-agent/enroll → 桌面端写 MCP 配置
桌面写入: POST /v1/panel（或 MCP）→ token/能力校验 → 解码内容 → 更新内存
          → PostMessage(显示+重绘) → 面板 WM_PAINT
崩溃/被杀: supervisor 健康探测失败 → 5s 防抖 → 拉起 serve（保留 --open-panel 意图）
主动退出: 托盘退出 / /v1/shutdown / stop → 写 mode-agent.stop → agent 退出
          → supervisor 看到 stop 文件 → 删除并退出
```

## 失败语义

| 场景 | code | HTTP |
| --- | --- | --- |
| 未带/错误 token | `unauthorized` | 401 |
| `allowDesktopControl=false` | `capability_disabled` | 403 |
| 空内容 / 超长 / 非法 base64 | `invalid_argument` | 400 |
| 图片解码失败 | `invalid_argument` | 400 |
| 非 Windows 或 UI 未启用 | `capability_unavailable` | 501 |
| 请求体超限 | `payload_too_large` | 413 |

运行期（不经过 HTTP）：

- 端口被另一个 token 不同的 mode-agent 占用：supervisor 记录日志并弹一次警告，每 10 秒重试，
  不会把对方的 agent 当成自己。
- supervisor 单实例：第二个 `start` 只尝试打开面板，不启动第二套服务。
- 配置损坏：GUI 启动弹错误框；supervisor 启动失败写入 `supervisor.log`。

## 验收场景

1. 全新目录双击 `ModeAgent.exe` → 首次运行对话框；取消不落盘；确认后在 exe 同目录生成
   `mode-agent.json`，无控制台窗口。
2. 确认开机自启 → Run 项为 `"<exe>" start --config "<config>"`；托盘关闭自启后 Run 项删除。
3. 面板固定 300×400 客户区，无内容时显示 Matrix 代码雨动画；拖动边缘不能改尺寸；
   标题栏和托盘显示 Mode 图标。
4. 桌面端 MCP 调用 `remote_panel_show`（标题+文本）→ 面板显示并渲染；`remote_panel_clear`
   → 恢复默认代码雨动画；发送 PNG → 按比例显示，文本在图片下方。
5. 在 Task Manager 结束 `serve` 子进程 → supervisor 在数秒内拉起新子进程，端口恢复健康；
   若启动时带 `--open-panel`，新面板重新显示。
6. 结束 supervisor → 已有 agent 继续服务；再次运行 exe 会重新接管（单实例、不重复监听）。
7. 托盘「退出」/`stop` → agent 与 supervisor 都退出；4 秒内不再被拉起。
8. 关闭 `allowDesktopControl` → 面板接口返回 `capability_disabled`，已有窗口不崩溃。
9. 关闭托盘图标 → 图标消失；再次运行 exe 仍可打开面板，并从原生系统菜单恢复托盘。
10. `mode-agent version` 在 cmd/PowerShell 有输出；`Start-Process -RedirectStandardOutput`
    能拿到重定向输出；双击 exe 不出现控制台。
11. Go 1.10.8 与现代 Go 均可编译；`build.ps1` 产出四个 Windows 产物且子系统=2、图标可提取。
12. 可选安装路径：`ModeAgent-Setup.exe` 双击进入安装对话框；`install --quiet` 从控制台
    运行时能输出安装信息。
13. 免安装首次运行 → 启动后自动弹出配对页；取消后 agent 继续健康；之后可从标题栏图标
    菜单「配对...」重新打开。
14. 设置页只显示自启/托盘两个开关；面板固定 300×400、不显示开关；关闭托盘后重新勾选
    立即恢复；自启从未开状态保存不会删除其他实例的 Run 项。
15. 配对页只接受 6 位配对码：空输入提示；同 LAN/Tailscale 有桌面端时自动发现并登记；
    无桌面端时给出明确提示；完整配对串仍可通过 CLI `enroll --uri` 使用。
