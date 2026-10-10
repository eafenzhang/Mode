# Computer Use（电脑控制）工具面规范

智能体通过共享 `node_repl` 宿主驱动本机桌面：观察（截图 / 无障碍树）→ 动作（鼠标、键盘、
元素操作）→ 再观察验证。运行时是自研 `@mode/cua` runtime + Helper（Windows 一期），
本文件是 14 个方法的参数与语义规范源。

## 前置条件

- 设置 → 基础设置 →「电脑控制」开关已开启（否则插件不加载，node_repl 不注册 CUA 能力）。
- 平台：**Windows 一期可用**；macOS / Linux 当前 fail-closed（`CUA_NOT_READY`，能力未开放）。
- 仅主会话可用：子代理（subagent）里调用会被拒绝。
- 调用方式：见 `skills/computer-use/SKILL.md`——通过
  `globalThis[Symbol.for("mode.node-repl.computer-use-bridge")]` 的 `call(method, input)`
  发起，或从插件根动态 import `scripts/computer-use-client.mjs` 使用 SDK 封装。

## 会话模型

- **观察→动作闭环**：动作的元素索引必须来自对该 app **最新一次** `get_app_state` 观察；
  未观察就动作 → `STALE_STATE`（本地拒绝）；观察后元素消失 → `element_unavailable`，
  重新观察再动。
- **controller lease**：变更类调用按 `(session_id, workspace_key)` 抢占租约；其它会话
  （异 session 或异 workspace）争用 → `controller_busy`，message 内含 owner。只读四方法
  （`get_app_state` / `list_apps` / `list_windows` / `request_access`）不进闸门。
- **停止闸门**：`stop_computer_control` 释放租约并置 stopped；之后变更类调用 →
  `controller_busy`，`stop` 自身幂等、可重复调用成功。
- 观察在采集前有实现侧的有界等待（settle），模型不需要自行 setTimeout 轮询。

## 通用参数

### `app_ref`

`{name | bundle_id | pid, window_id?}`，裸字符串按 `bundle_id` 解析。

- Windows：`name` 是 OS 列出的显示名（Start 菜单名），**不是窗口标题**；
  `bundle_id` 优先 AUMID，回退可执行文件路径。
- 解析未命中运行中应用时**透明启动**：先后台拉起再有界轮询 `list_apps` 回填 pid；
  启动失败保留 `target app is not running` 语义供换字段重试。`{pid}` 直连不走透明启动
  （死 pid 无从启动）。
- `window_id` 限定目标窗口；缺省取该 app 主窗口。

### `target`

- `{type:"element", index}`：索引身份 = app + index，必须来自该 app 最新一次观察。
- `{type:"coordinate", x, y, frame_id?}`：省略 `frame_id` 时绑定会话最近一次可动作栅格；
  无栅格或栅格过期 / owner 不符 → `invalid_request`。

## 方法参考（14）

### 观察与枚举（只读）

- **`get_app_state`** — 取无障碍树文本 + 可选截图。参数仅四键（strict，未知键
  `invalid_request`）：`app_ref`、`include_screenshot`、`disable_diffing`、
  `tree_shown_to_model`。diff 基线 = 该会话已展示给模型的最近树；首观察与截图后首观察
  必为全量。树格式：首行 `app: <name> pid=<N> "<title>"`，元素行 `[<idx>] …` 带
  `name = value` 与该元素 `actions` 列表；元素数达上限时树头附
  `showing A-B of N items` 提示。`include_screenshot` 截图前先做锁屏预检，锁屏 →
  `permission_denied` 且**零截图**。
- **`list_apps`** — 枚举运行中与可启动的应用，为 `app_ref` 提供 name / bundle_id / pid
  依据。
- **`list_windows`** — 窗口清单：`index`、`window_id`、`title`、`bounds`、`main`、
  `focused`、`onscreen`（`subrole` / `text_preview` 为 macOS 字段，Windows 不产出）。
- **`request_access`** — 查询 / 请求平台能力。Windows 无 TCC：文本块为扁平状态
  （全部 granted / not_required），`structuredContent` 形如
  `{platform:"windows", backend:"uia", accessibility:{status_after:"not_required"},
screen_recording:{status_after:"not_required"}}`。

### 动作（变更类，受租约与停止闸门约束）

- **`left_click`** — 点击 `target`（元素索引或坐标）。
- **`scroll`** — 在 `target` 处滚动；`scroll_amount` 为页数，clamp 0–100。
- **`left_click_drag`** — 从起点拖到终点（坐标对）。
- **`type`** — 向聚焦处输入文本。
- **`key`** — 按键：xdotool 风格 keysym + 短名（`Return` / `Tab` / `Control_L+a` /
  `super+c` / `Up`…；Windows 可用 `ctrl`），支持 `repeat`、`hold_seconds`。
- **`set_value`** — 直接写元素值：优先无障碍 ValuePattern（后台可用）；不支持 →
  聚焦 + 全选 + `type` 的 event 兜底；均不可 → `not_settable`。
- **`select_text`** — 选区操作：TextPattern `Select(range)`；元素不支持 →
  `not_selectable`（歧义匹配拒绝，不取第一个）。
- **`perform_action`** — 执行元素 `actions` 列表中**已列出**的动作；未列出 →
  `action_unavailable`。
- **`paste`** — 写系统剪贴板 → 粘贴 → 恢复用户剪贴板；无 app 消费 → `timeout`；
  后台 app → `foreground_required`（paste 走 event 路径）。
- **`stop_computer_control`** — 释放租约、停止本会话控制；幂等。

### 动作策略与前台规则

- `strategy:"event"` 要求目标 app（与 `window_id`）已前台，否则 `foreground_required`
  且**不下发任何输入**；`"auto"` 先无障碍后 event。
- 元素索引动作前若元素已不在最新树 → `element_unavailable`（建议：重新
  `get_app_state` 再动）。

## 错误与收据

错误形态：文本 JSON `{code, message, suggested_action?, details?}`，同时
`structuredContent.error` 携带 `{code, suggested_action?, details?}`（message 不进
structuredContent）。关键事实（如 lease owner）一定在 message 文本里。

常见码与处置：

| code                                                     | 含义 / 处置                                                                           |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `CUA_NOT_READY`                                          | Helper 冷启动中（message 固定提示稍等后**重试同一调用**）；可重试                     |
| `STALE_STATE`                                            | 动作前未观察或观察过期；重新 `get_app_state`                                          |
| `element_unavailable`                                    | 元素已消失；重新观察（suggested: Re-observe with get_app_state before acting again.） |
| `foreground_required`                                    | 需要前台；改用元素索引或先将 app 带到前台                                             |
| `controller_busy`                                        | 租约被其它会话占用或本会话已 stop；观察现状或询问用户                                 |
| `permission_denied`                                      | 锁屏等系统边界，零副作用拒绝                                                          |
| `not_settable` / `not_selectable` / `action_unavailable` | 元素能力不支持，换路径                                                                |
| `invalid_request`                                        | 参数 / 校验失败（含 `get_app_state` 未知键、栅格身份不匹配）                          |
| `timeout`                                                | 动作超时（如 paste 无消费）                                                           |
| `version_mismatch`                                       | Helper addon 协议版本不一致，never-retry，需升级环境                                  |

- **收据**：变更类动作可能返回 `dispatched` 三态收据（收据与 error 同层出现在
  `structuredContent` 顶层）。动作已发出但报错时**绝不盲重放**——按「重新观察」处理。
- 健康预检 5s、单次 broker 调用 30s 超时。

## 安全边界

- 涉及付款、注册、删除、密码 / 凭据输入的操作必须先向用户确认；不得把密钥写进日志或
  会话外渠道。
- 锁屏一律 `permission_denied`，不尝试绕过。
- 观察截图可能含敏感窗口内容：只在任务需要时请求 `include_screenshot`。
