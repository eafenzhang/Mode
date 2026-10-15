# Spec：Tauri 平台命令面（M1 阶段）

前置：路线图 `docs/plans/2026-10-09-tauri-rewrite-roadmap.md` M1 节；
执行计划 `docs/plans/2026-10-15-tauri-skeleton.md`。
纪律：`docs/specs/clean-room-policy.md`——`crates/**` 与 `apps/desktop-tauri/**` 全部适用；
`packages/shared/src/platform.ts` 属上游衍生文件，**只允许读接口面**（签名 / 类型声明 /
文档注释），禁读实现体。

## 0. 范围与所有权

M1 的 Tauri 壳**只做平台命令面**：业务逻辑（会话运行时、任务状态、插件宿主）仍由既有
Node/TS 侧提供，经 sidecar（M3）接入。因此本阶段：

- Rust 侧不持有任何业务真源；
- 命令面之外的一切（`onFocusTab`、`onNewTab`、权限审批、任务队列…）不在 M1 范围；
- `crate::AppState` 只保存「最近一次由渲染层同步进来的值」，是骨架期的占位所有者，
  M3 接线后交还给真正的所有者（见 `crates/platform/src/state.rs` 的模块文档）。

## 1. 命令切片

M1 只实现下列 10 个 `#[tauri::command]`。命令名与 `IPlatformService`
（`packages/shared/src/platform.ts`）**逐字一致**——渲染层的适配层按同名映射，
改名即断链。切片表的唯一真源是 `crates/platform/src/commands/mod.rs` 的宏调用：
命令名与处理函数成对声明，`init()` 与 `registered_names()` 都从它派生，
并由 `crates/platform/tests/commands.rs` 守卫「JS 名 = 处理函数名的 camelCase 形式」。

| # | 方法 | 签名（来自接口面） | M1 实现口径 |
| --- | --- | --- | --- |
| 1 | `openExternal` | `(url: string) -> void` | `tauri-plugin-opener.open_url`；只拒空串，**不收紧 scheme**（收紧会改现网行为，属单独加固决策） |
| 2 | `selectDirectory` | `() -> Promise<string \| null>` | `tauri-plugin-dialog` `pick_folder`；取消 → `null` |
| 3 | `selectFile` | `() -> Promise<string \| null>` | `pick_file`；取消 → `null` |
| 4 | `saveFile` | `(payload: SaveFileRequest) -> Promise<SaveFileResult>` | **部分实现**：`data` 分支完整；`sourceUrl` 分支需 HTTP 取回，M1 无 HTTP 客户端，明确返回 `error: "save_file_source_url_not_implemented_until_m3"`（不静默写空文件） |
| 5 | `openInFileManager` | `(path: string) -> Promise<{success, error?}>` | 按路径类型分派：目录 → 文件管理器打开；文件 → 在父目录中选中；路径不可用 → `success: false` |
| 6 | `showTaskNotification` | `(payload: TaskNotificationPayload) -> void` | `tauri-plugin-notification`；是否真正展示交系统 |
| 7 | `notifyRendererReady` | `() -> void` | 记录就绪位到 `AppState`（deep link 转发在 Task 6） |
| 8 | `syncWindowTabs` | `(paths: string[]) -> void` | 覆盖式写入 `AppState` |
| 9 | `syncWindowUnreadCount` | `(count: number) -> void` | 覆盖式写入 `AppState` |
| 10 | `getApplicationIcon` | `(req: ApplicationIconRequest) -> IconInfo \| null` | **部分实现**：命令可调用，一律返回 `null`。契约允许 `null` 且调用方已处理；真实取值需按平台做原生图标解码，见 §5 |

### 非本地路径的处理

`selectDirectory` / `selectFile` 的返回值契约是「agent 可访问的**本地绝对路径**」，
故 `FilePath::Url`（如 Android `content://`）被当作取消处理，不把 URI 当路径返回。

## 2. 事件通道

订阅型方法（`onFocusTab` / `onNewTab` / `onOAuthCallback` / `onShareImport` …）在 M1 只落
**一个**示范：

- `onOAuthCallback`：Tauri event + deep-link（`tauri-plugin-deep-link`，scheme 沿用既有
  `mode`，见 `packages/desktop/electron-builder.config.js` 的 `protocols`）。
  Rust 侧在收到 `mode://` 回调时 `emit` 一个事件，渲染层适配层把它接到
  `onOAuthCallback` 的订阅语义上（Task 6 接线）。

其余订阅型方法**留 M3**：它们依赖 sidecar 提供的事件源，M1 没有可发的事件。
适配层对未实现的方法必须**显式抛出** `not_implemented_until_m3`，不得静默返回假值或空订阅
（见 `packages/ui/src/platform/tauri-invoke-shim.ts` 的测试）。

## 3. capability 规则

`apps/desktop-tauri/src-tauri/capabilities/default.json` 当前**只开 `core:default`**。

这不是省略，而是当前信任边界的正确表达：

- capability 管的是**渲染层 → 插件/核心命令**的 IPC 面。本切片的 10 个命令都是本应用自己
  的 `#[tauri::command]`，在 **Rust 侧**调用 `tauri-plugin-dialog` / `opener` / `notification`
  的 Rust API（`app.dialog().file().pick_folder(...)` 等），**不经过渲染层的插件命令**
  （渲染层从不 `invoke("plugin:dialog|open")`）。因此这些插件的 `allow-*` 权限现在没有调用方。
- 实证：`tauri-build` 在 capabilities 里列 `dialog:allow-open` 等会直接报
  “Permission dialog:allow-open not found”——插件不在 app 的依赖图里，权限清单也就没有它。

规则：

- 不使用 `*` 通配权限；**当渲染层开始直接调用某个插件命令时**，才新增该插件到
  `apps/desktop-tauri/src-tauri/Cargo.toml` 依赖并加上对应的最小 `allow-*` 权限；
- capability 的 `windows` 只列 `main`（`tauri.conf.json` 里窗口 label 为 `main`）；
- 文件系统权限（`fs:*`）本阶段**一律不开**——本切片的文件写入只发生在
  `saveFile` 用户显式选定的路径上，由 Rust 侧直接写，不经前端 fs 插件。

## 4. 验收场景

**E2E：开窗 → 渲染 UI → 10 命令全绿**

1. 构建壳：`cargo build -p mode-tauri`（或 `tauri build`），启动后主窗口可见；
2. 前端挂载（Task 4）后，`window` 上存在 `invoke` 适配层；
3. 依次触发 10 个命令，逐条判据：

| 命令 | 通过判据 | 失败判据 |
| --- | --- | --- |
| `openExternal` | 系统默认浏览器打开测试 URL | 静默无反应，或抛错 |
| `selectDirectory` | 选择框弹出；选中后返回值 = 所选绝对路径；取消返回 `null` | 返回值是 URI，或卡死 |
| `selectFile` | 同上（文件） | 同上 |
| `saveFile`（`data`） | 选定路径后文件落盘且字节一致；取消返回 `canceled: true` | 取消时 `success: true`；或写入 0 字节 |
| `saveFile`（`sourceUrl`） | 返回 `success: false` + `save_file_source_url_not_implemented_until_m3` | 返回 `success: true` |
| `openInFileManager`（目录） | 文件管理器打开该目录 | — |
| `openInFileManager`（文件） | 文件管理器打开父目录并选中该文件 | 打开了文件本身（不是"在文件管理器里打开"） |
| `openInFileManager`（不存在） | `success: false` + `path_unavailable` | `success: true` |
| `showTaskNotification` | 系统通知出现，标题/正文与入参一致 | 抛错 |
| `notifyRendererReady` | 命令返回成功 | 抛错 |
| `syncWindowTabs` | 命令返回成功 | 抛错 |
| `syncWindowUnreadCount` | 命令返回成功 | 抛错 |
| `getApplicationIcon` | 返回 `null`（M1 口径） | 抛错 |

4. `cargo clippy -D warnings` 零告警；`cargo test` 全绿（含 §1 的名字守卫）；
5. 安装包内不含 Node 运行时。

**自动化覆盖边界**：命令注册表、名字映射、协议往返、`SaveFileRequest` 不变量、
`AppState` 写入是自动化的；上表里需要真人看窗口/对话框的项属**人工核对**，
无人值守环境（CI）跑不了，记录在 PR 里。

### 4.1 测试用例的位置是被约束的（别随手挪）

`crates/platform/tests/commands.rs` **必须留在 `tests/`（集成测试目标）**，不能挪进
lib 的单测模块。原因是一条 Windows 链接约束链：

1. 本 crate 依赖 tao/tauri，其 Windows 路径用到 comctl32 **v6** 才有的 `TaskDialogIndirect`；
2. v6 只有带应用清单的可执行文件才加载，否则启动即 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)`；
3. app 二进制的清单由 `tauri_build::build()` 注入，**cargo 的测试二进制没有**；
4. `crates/platform/build.rs` 只对**测试目标**注入清单，方式是 `cargo:rustc-link-arg-tests`
   —— 而 cargo 只把 `tests/` 下的目标算作测试目标（放进 lib 单测会报
   “The package … does not have a test target”，且 lib 单测目标拿不到清单）。

该文件还说明了为什么不能用 `tauri-winres` / `embed-resource`：crate 没有 `[[bin]]` 时它们走
link-lib 兼容分支，资源库会**传播给依赖方**，与 app 自己的清单撞车
（`duplicate resource: type MANIFEST (ID 24)`）；且其 `write_resource_file` 无条件写
VERSIONINFO，连"让 platform 出清单、app 去掉自己的"这条退路也堵死。

## 5. M1 未完成项（明确登记，不假装完成）

| 项 | 原因 | 归口 |
| --- | --- | --- |
| `getApplicationIcon` 的原生图标解码 | 需按平台取 exe / AUMID / bundle 的图标资源并编码为位图数据 URL（Windows GDI、macOS `NSWorkspace`），是一件独立于命令面注册的活 | M3（与 CUA 展示面一起） |
| `saveFile` 的 `sourceUrl` 分支 | 需要一个 HTTP 客户端与取回超时/大小上限的产品规则 | M3（sidecar 接入后） |
| `onOAuthCallback` 之外的订阅型方法 | 依赖 sidecar 的事件源 | M3 |
| 三平台出包与体积基线 | 需 CI 跑 `tauri build`，本机只能出 Windows | M1 Task 5（CI） |

## 6. 协议双端生成

- Rust 侧 `crates/protocol` 是 `SaveFileRequest` / `SaveFileResult` /
  `TaskNotificationPayload` / `ApplicationIconRequest` 等类型的**真源**，
  带 `#[derive(schemars::JsonSchema, Serialize, Deserialize)]`；
- 导出方向单向：Rust → TS（`packages/shared/src/generated/tauri-protocol.ts`），
  由 `scripts/generate-tauri-protocol.mjs` 生成，CI 断言生成物无漂移；
- 字段名逐一对齐 TS：TS 是 camelCase，故 Rust 结构体一律
  `#[serde(rename_all = "camelCase")]`；
- `SaveFileRequest` 在 TS 侧是「`data` 与 `sourceUrl` 二者恰有其一」的联合，
  JSON 载体表达不了字段级排除，故 Rust 侧用 `Option` 表示两个字段并在
  `validate()` 里把该不变量显式化，由命令面在入口拒绝违反者。

## 7. 与既有语义的关系

本阶段只打底，**不实现运行时**。以下既有语义在 M1 不得被削弱或替代：
owner/lease、`CommandInbox` 串行准入、`desktop-continuous` 与 `web-remote-replayable`
双链路、`workspaceIdentity` 贯穿。M1 的命令面里没有任何一条会写这些状态
（`AppState` 是骨架期占位，见 §0）。
