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

M1 的 10 条命令 + LR-3 追加的 `rdReadClipboardText` + LR-5 追加的
`createTempTextAttachment` / `openExternalFile` / `exportLogs`（共 14 条）。命令名与 `IPlatformService`
（`packages/shared/src/platform.ts`）**逐字一致**——渲染层的适配层按同名映射，
改名即断链。切片表的唯一真源是 `crates/platform/src/commands/mod.rs` 的宏调用：
命令名与处理函数成对声明，`init()` 与 `registered_names()` 都从它派生。

**每个处理函数必须自己写一遍 `#[tauri::command(rename = "<切片表里的同一名字>")]`。**
这不是冗余，是这条契约能成立的唯一方式：

- Tauri v2 的命令名**默认是函数名的 snake_case 原文**（`tauri-macros` 的
  `command/wrapper.rs`：无 `rename` 时取 `stringify!(ident)`），**不会自动转 camelCase**，
  且这一版没有 `rename_all`；handler 是按 `#[tauri::command]` 生成的
  `__tauri_command_name_*` 宏的返回值做 `match` 的。
- 也就是说 wire 名**不存在于任何 Rust 常量里**，只有源码能证明它。少了 `rename`，
  `invoke("openExternal")` 命不中 `open_external` 分支，运行时才报
  `Command openExternal not found`（见 §4.3 的实测）。
- 因此守卫不能只比两个常量：`crates/platform/tests/commands.rs` 的
  `every_command_declares_a_camel_case_wire_name` **扫源码**，逐条断言
  `rename` 字面量 == camelCase(函数名)，并断言其集合等于切片表。去掉任一 `rename` 即红。

| # | 方法 | 签名（来自接口面） | M1 实现口径 |
| --- | --- | --- | --- |
| 1 | `openExternal` | `(url: string) -> void` | `tauri-plugin-opener.open_url`；只拒空串，**不收紧 scheme**（收紧会改现网行为，属单独加固决策） |
| 2 | `selectDirectory` | `() -> Promise<string \| null>` | `tauri-plugin-dialog` `pick_folder`；取消 → `null` |
| 3 | `selectFile` | `() -> Promise<string \| null>` | `pick_file`；取消 → `null` |
| 4 | `saveFile` | `(payload: SaveFileRequest) -> Promise<SaveFileResult>` | **已实现（M3 Task 4 起）**：`data` 分支与 `sourceUrl` 分支都完整；`sourceUrl` 走**有上限**的取回（超时 30s / 字节上限 64 MiB，见 §9.3），失败**不写任何字节**。M1 的占位错码 `save_file_source_url_not_implemented_until_m3` 已作废 |
| 5 | `createTempTextAttachment` | `(payload: CreateTempTextAttachmentRequest) -> Promise<CreateTempTextAttachmentResult>` | **已实现（LR-5，2026-10-22）**：写入系统临时目录 `mode-temp-attachments/<pid>-<纳秒>/<filename>`；文件名去路径分量、非法字符替换为 `_`，空名/`.`/`..` 回退 `attachment.txt`；每次调用独立子目录，同名不覆盖。失败返回稳定码 `create_temp_text_attachment_failed`，IO 细节只进 stderr |
| 6 | `openInFileManager` | `(path: string) -> Promise<{success, error?}>` | 按路径类型分派：目录 → 文件管理器打开；文件 → 在父目录中选中；路径不可用 → `success: false` |
| 7 | `openExternalFile` | `(path: string) -> Promise<{success, error?}>` | **已实现（LR-5，2026-10-22）**：用系统默认应用打开**存在的文件**；目录返回 `not_a_file`（目录走 `openInFileManager`），不存在返回 `path_unavailable: …`。校验抽成纯函数，用例覆盖不存在/目录/文件三种形态 |
| 8 | `showTaskNotification` | `(payload: TaskNotificationPayload) -> void` | `tauri-plugin-notification`；是否真正展示交系统 |
| 9 | `notifyRendererReady` | `() -> void` | 记录就绪位到 `AppState`；**冷启动深链在这里回放**（缓存与回放时序在 `crates/deeplink` 的 router，有单测）。运行期与第二实例转发见 §2 与 §9.1 |
| 10 | `syncWindowTabs` | `(paths: string[]) -> void` | 覆盖式写入 `AppState` |
| 11 | `syncWindowUnreadCount` | `(count: number) -> void` | 覆盖式写入 `AppState` |
| 12 | `getApplicationIcon` | `(req: ApplicationIconRequest) -> IconInfo \| null` | **已实现（Windows，M3 Task 4 起）**：Shell 取图标资源 + GDI 取像素 → PNG data URL；按 locator 顺序尝试，**取不到返回 `null`（契约允许），从不抛**。**macOS 半边未实现且本阶段不验**（Ruling 20），见 §9.2 |
| 13 | `rdReadClipboardText` | `() -> Promise<string>` | **已实现（LR-3，Windows）**：壳读 CF_UNICODETEXT（`OpenClipboard` 争用重试 200ms，无文本/被占用返回空串，从不抛）；非 Windows 返回空串。远程桌面主控剪贴板同步的读取口（D7：Host 不碰平台 API） |
| 14 | `syncAppSettings` | `(patch: Partial<AppSettings>) -> void` | **已实现（2026-10-25）**：窗口行为类设置的壳侧同步口。壳只解释它执行的键（现 `closeToTrayOnWindows`，落 `WindowCloseBehavior` AtomicBool），其余键不解释；缺省 `false` = 直接退出（fail-safe）。**关闭拦截**在壳 `main.rs` 的 `CloseRequested` 统一做：开关为开且托盘可用（`tray::TRAY_ID` 可查）→ `prevent_close` + `hide`（自绘关闭按钮/Alt+F4/任务栏三条路径全汇聚于此）；托盘不可用即使开关为开也放行关闭——「隐藏到不存在的托盘」等于应用凭空消失。开关事实由渲染层同步：启动时一次（`useRootPlatformEffects`，Windows 门控）+ 设置页变更补丁；壳不读业务存储。补丁当前消费两个键：`closeToTrayOnWindows`（见上）与 `keepAwakeWhileRunning`（**保持电脑运行**：`SetThreadExecutionState(ES_CONTINUOUS\|ES_SYSTEM_REQUIRED)` 阻止空闲休眠、手动睡眠/合盖不受影响；设置页同门控仅 Windows 呈现，非 Windows 执行器 no-op） |

| 15 | `exportLogs` | `() -> Promise<{ success: boolean; path?: string; error?: string }>` | **已实现（2026-10-26，LR-5 §27）**：把 Host sidecar 的文件日志打成一个**单文件文本包**，经系统保存对话框落盘。**日志目录与 Host 同口径解析**：数据基目录 = `MODE_DATA_BASE_DIR`（非空）→ 默认根 `~/.mode/v2/setting.json` 的持久化 `dataBaseDir` 指针 → 家目录；日志目录 = `<基目录>/.mode/v2/logs`（壳是 Host 的父进程，两处输入一致；纯函数在 `sidecar_host::settings_probe`，含用例）。内容 = 该目录下全部 `*.log`（按名升序），包格式见 §1.1。**不引入压缩依赖**（单文件可直读、三平台一致）。对话框归属壳，默认文件名 `Mode-logs-YYYYMMDD-HHMMSS.log`；**用户取消 → `{success:false}` 且无 `error`**（取消不是失败）；无日志可导 → `{success:false, error:"no_logs"}`（不导出空文件冒充成功）；写盘失败 → `{success:false, error:"write_failed"}`，细节只进 stderr。**UI 入口**（2026-10-26 恢复）：帮助菜单「导出日志」→ `platform.exportLogs()`，成功 toast 带落盘路径、取消静默、失败显示错误码（`WorkspaceHelpMenuButton`） |

### 1.1 `exportLogs` 的包格式

```
Mode logs export
exportedAt: <RFC 3339 UTC>
logDir: <绝对路径>
files: <数量>

===== <文件名> (<字节数> bytes) =====
<文件原文（非 UTF-8 字节按 lossy 替换）>

===== <下一个文件> … =====
```

行为确定：文件按名称升序拼接、每段之间以空行分隔、末尾补一个换行。写盘不承诺原子性
（诊断导出而非业务数据；失败返回 `write_failed`，可能留下不完整文件）。

### 非本地路径的处理

`selectDirectory` / `selectFile` 的返回值契约是「agent 可访问的**本地绝对路径**」，
故 `FilePath::Url`（如 Android `content://`）被当作取消处理，不把 URI 当路径返回。

## 2. 事件通道

**深链事件通道**（M1 落地、M3 Task 4 接线完成）：Tauri event + deep-link
（`tauri-plugin-deep-link`，scheme 沿用既有 `mode`，见
`packages/desktop/electron-builder.config.js` 的 `protocols`）。Rust 侧把解析结果 `emit` 成
**一个**事件（`mode:deep-link`，事件名与 `kind` 取值都是 `crates/protocol` 的生成物），
渲染层按载荷的 `kind` 分流。

**五种 `kind` 现在都有订阅点**（`crates/deeplink` 一直投递全部五种，M1 只接了前两条）：

| `kind` | 订阅方法 | 交给回调的东西 |
| --- | --- | --- |
| `oauth-callback` | `onOAuthCallback` | URL 原文 |
| `payment-callback` | `onPaymentCallback` | URL 原文 |
| `share-import` | `onShareImport` | `{ shareCode }`（M3 Task 4 接线） |
| `workspace-open` | `onOpenWorkspacePath` | 解析后的 `path`（M3 Task 4 接线） |
| `unrecognized` | 无订阅点（**刻意**：它是「宿主收到了但没人认领」的观察面） | — |

其余订阅型方法（`onFocusTab` / `onNewTab` / `onNewTask` / `onWindowFullscreenChanged` /
`onTaskNotificationClick` / `onRemote*` / 更新链的那几条 …）**逐条登记在 §9.4**：
它们的事件源不是深链（菜单/托盘/远控/更新链），本阶段没有可发的事件，所以按声明返回形态
降级为「被拒 Promise」或「留痕空操作」——**不静默返回假值或空订阅**
（见 `crates/sidecar-host/src/window_mode.ts` 的两类清单与
`apps/desktop-tauri/test/windowModeInjection.test.mts` 的机械对拍）。

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

1. 构建壳：`pnpm --filter @mode/desktop-tauri exec tauri build`（或本机 `cargo run -p mode-tauri`），
   启动后主窗口可见；

   > **本机（Windows + gnu）不要直接调 cargo**：本机默认 toolchain 的 host 是 msvc，而
   > 机器上没装 VS Build Tools（Git Bash 自带的 coreutils `link` 还会挡住 MSVC 的
   > `link.exe`），会以 `dlltool.exe: program not found` 或 `link: extra operand` 失败。
   > 工具链装配口径收敛在 `scripts/rust-gnu-env.ts`（唯一出处），用包装器一条命令即可
   > ——CI 是 msvc / Linux，装配在那边退化成 no-op，直接调 cargo 即可，不必经它：
   >
   > ```bash
   > node scripts/rust-cargo.ts build --release -p mode-tauri   # 等价于 mise run rust -- build --release -p mode-tauri
   > node scripts/rust-cargo.ts test --workspace
   > ```
   >
   > 产出落在 `target/release/ModeTauri.exe`：包装器刻意**不注入 `--target`**，按宿主
   > toolchain 构建（宿主被换成 gnu），所以落在 `target/<profile>` 而不是
   > `target/<triple>/<profile>`。
2. 前端挂载（§4.2）：窗口里渲染出 M1 命令面验收台，深链订阅已装好并在启动时发出握手；
3. 依次触发 10 个命令，逐条判据：

| 命令 | 通过判据 | 失败判据 |
| --- | --- | --- |
| `openExternal` | 系统默认浏览器打开测试 URL | 静默无反应，或抛错 |
| `selectDirectory` | 选择框弹出；选中后返回值 = 所选绝对路径；取消返回 `null` | 返回值是 URI，或卡死 |
| `selectFile` | 同上（文件） | 同上 |
| `saveFile`（`data`） | 选定路径后文件落盘且字节一致；取消返回 `canceled: true` | 取消时 `success: true`；或写入 0 字节 |
| `saveFile`（`sourceUrl`） | 选定路径后从远端取回并落盘（字节一致）；超时/超上限时 `success: false` + 稳定错码（`save_file_source_url_timeout` / `…_too_large`），且**不产生文件**；取消仍返回 `canceled: true` | 超上限仍返回 `success: true`；或失败时留下半截文件 |
| `openInFileManager`（目录） | 文件管理器打开该目录 | — |
| `openInFileManager`（文件） | 文件管理器打开父目录并选中该文件 | 打开了文件本身（不是"在文件管理器里打开"） |
| `openInFileManager`（不存在） | `success: false` + `path_unavailable` | `success: true` |
| `showTaskNotification` | 系统通知出现，标题/正文与入参一致 | 抛错 |
| `notifyRendererReady` | 命令返回成功 | 抛错 |
| `syncWindowTabs` | 命令返回成功 | 抛错 |
| `syncWindowUnreadCount` | 命令返回成功 | 抛错 |
| `getApplicationIcon` | Windows：取到 `data:image/png;base64,…`（真实图标）或 `null`（取不到）；**任何情况下都不抛** | 抛错，或返回空串/非 PNG 载体的 data URL |

4. 深链通道（M1 唯一接线的事件链）：用 `mode://oauth/callback?state=test` 打开应用，
   验收台的「深链回调」区出现该 URL；
5. `cargo clippy -D warnings` 零告警；`cargo test` 全绿（含 §1 的名字守卫）；
6. 安装包内不含 Node 运行时。

   > **这一条是 M1 阶段口径，对「产品阶段」已被修订**（2026-10-16）。它描述的是 M1 骨架的真实
   > 状态——前端是纯静态资源，壳体确实没有 Node 运行时——对此仍然成立。但真实 UI **必须**由
   > Host 提供 `IServiceAccessor` 才能挂载（§4.2 的证据表），而 Host 跑的是存量 TS services 层
   > （含 `node:sqlite`/`node-pty` 原生依赖），所以**产品阶段必然携带 Node 运行时**。
   > 修订后的口径与依据见 `docs/plans/2026-10-16-tauri-m3-ecosystem.md` 的 **DR-M3-1**：
   > 产品阶段的对应门禁改为 **M3 的体积门禁（核心 ≤30MB）+ 实测 Node 运行时的落点与代价**。
   > 若「产品不得带 Node」是硬约束，唯一替代是把 Host/services 层也用 Rust 实现——那等于新增一个
   > M2 量级的阶段，需要重新决策。

**自动化覆盖边界**：命令注册表、名字映射、协议往返、`SaveFileRequest` 不变量、
`AppState` 写入、深链解析与投递时序、适配层的命令映射与订阅分流是自动化的；
上表里需要真人看窗口/对话框的项属**人工核对**，无人值守环境（CI）跑不了，记录在 PR 里。

### 4.1 测试用例的位置是被约束的（别随手挪）

`crates/platform/tests/commands.rs` **必须留在 `tests/`（集成测试目标）**，不能挪进
lib 的单测模块。原因是一条 Windows 链接约束链：

1. 本 crate 依赖 tao/tauri，其 Windows 路径用到 comctl32 **v6** 才有的 `TaskDialogIndirect`；
2. v6 只有带应用清单的可执行文件才加载，否则启动即 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)`；
3. app 二进制的清单由 `tauri_build::build()` 注入，**cargo 的测试二进制没有**；
4. 测试目标的清单靠 `cargo:rustc-link-arg-tests` 注入——而 cargo 只把 `tests/` 下的目标
   算作测试目标（放进 lib 单测会报 "The package … does not have a test target"）。

该逻辑已抽成构建依赖 `crates/tauri-test-manifest`（`crates/deeplink` 的集成测试目标命中
同一个坑），各 crate 的 `build.rs` 只剩一行调用。该 crate 的模块文档解释了为什么不能用
`tauri-winres` / `embed-resource`：crate 没有 `[[bin]]` 时它们走 link-lib 兼容分支，资源库会
**传播给依赖方**，与 app 自己的清单撞车（`duplicate resource: type MANIFEST (ID 24)`）。

### 4.2 前端挂载：M1 挂的是命令面验收台，不是存量 renderer（Deviation）

计划原文（Task 4 Step 2b）写的是「`frontendDist` 指向 renderer 产物」。**实测做不到**，
改成 `apps/desktop-tauri/frontend/`（Vite + TS 的验收台）。依据是存量 renderer 的启动链：

| 依赖 | 证据（`packages/desktop/src/renderer/main.tsx`） | 归属 |
| --- | --- | --- |
| Electron preload 的 `window.mode` | `createDesktopPlatform({ isLocalDevelopmentRuntime })` 直接建桌面平台服务 | 适配层可替（已做） |
| **Node Host 经 MessagePort 提供 `IServiceAccessor`** | `connectViaMessagePort(...)`；注释原文「preload 用 window.postMessage + transfer 把 port 原样传递到 renderer」「本地窗口的 port 来自 utilityProcess」 | **M3（DR-01 sidecar）** |
| Host 的数据库启动通道 | `DatabaseStartupAdmission`，30s 内没收到 `startup_status` 就渲染启动失败屏 | **M3** |

也就是说：光注入 `window.mode` 不够——没有 Host 时渲染进程会停在启动/失败屏。
把那个结果当「渲染 UI」交付是假的，所以 M1 交付**真实可用的命令面验收台**
（渲染 10 条命令的触发入口与结果、订阅深链事件并显示回调），把「渲染产品 UI」按上表
登记到 M3。验收台的命令表用 `satisfies` + 编译期守卫保证覆盖切片表每一条
（漏一条 `pnpm typecheck` 直接报错），不会出现「验收台绿了但命令面没验完」。

### 4.3 本机 smoke 执行记录（2026-10-16，**发现了三个自动化没覆盖的缺陷**）

按「Tauri 产物进 Release」之外的另一处人工项补跑：本机启动壳 + 深链端到端。
**结论：修完之后 §4 的第 1、2、4 项与命令通路实测通过；对话框/通知类仍需真人看窗口。**

观测手段（无人值守下怎么看渲染层）：WebView2 **不认**
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`（wry 自己设了 `additional_browser_args`，
环境变量被覆盖），CDP 接不上；改用 UI Automation 的 `TextPattern` 读文档文本，
锚点用 `AutomationElement::FromHandle(hwnd)`（从桌面根往下走常常只看到外层 Pane）。
脚本留在 `.plan/uia-doc.ps1`，人工验收可复用。

跑出来的三个缺陷（**都已修**，每个都补了能真正抓住它的守卫）：

| # | 症状 | 根因 | 为什么自动化没抓到 |
| --- | --- | --- | --- |
| 1 | 壳启动即 panic，窗口根本不出来 | `plugins.updater` 段缺席 → Tauri 传 `null` → `PluginInitialization("updater", "invalid type: null, expected struct Config")`。`tauri_plugin_updater::Config.pubkey` 是**必填**（无 `serde(default)`），「不设 pubkey」做不到字面意思 | 只跑过 `cargo build`（编译过）与 `cargo test`（不启 app），没有任何一步真的启动过窗口 |
| 2 | 10 条命令**全部** `Command X not found` | 见 §1：`#[tauri::command]` 默认按 snake_case 注册，切片表的 camelCase 名一个都命不中 | 旧守卫只比 `M1_COMMANDS` 与 `M1_HANDLER_PATHS` 两个常量，二者本来就自洽；wire 名不在常量里 |
| 3 | 面板恒显示 `(尚无回调)`，深链收不到 | **#2 的症状**：`notifyRendererReady` 的 invoke 也命不中 → 宿主收不到握手 → 冷启动队列永不回放（`crates/deeplink` 的 router 与单测都对，队列确实积压了 1 条） | router 的单测覆盖了缓存/回放时序，但没人验证过「渲染层能不能真的把握手发出来」 |

顺带核实的两个否定结论（都查过、都不是原因，避免下次重走）：

- **不是 capability 问题**：10 条命令是本应用的 `#[tauri::command]`，不需要 capability 条目。
  Tauri 的 `generate_handler!` 确实会在**应用 ACL 存在时**静默剔除未授权命令
  （`filter_unused_commands`，会 `println!` 一行 "Removed unused commands from …"），
  但那条路径只在 `build > removeUnusedCommands` 打开（CLI 设 `REMOVE_UNUSED_COMMANDS`
  环境变量）时才生效；本仓没开，`allowed-commands.json` 也不存在。
- **不是 `generate_handler!` 嵌套在 `macro_rules!` 里的问题**：`:path` 片段传进 proc macro
  能正常解析，handler 里注册的名字来自函数自己的 `__tauri_command_name_*` 宏，与嵌套无关。

### 4.4 2026-10-17 E2E 实测（M1 Task 4 Step 3）

重建壳与验收台后，以 `MODE_TAURI_SHELL_ENTRY=dev/index.html` 启动，用 UIA `TextPattern`
读数、`ScrollItemPattern` + 鼠标点击/`InvokePattern` 触发。逐条结果：

| 命令 | 实测 |
| --- | --- |
| `openExternal` | `(void)`；Edge 打开 `example.test` |
| `selectDirectory` | 取消 → `null`；选择：地址栏键入 `D:\ZCodium\.plan` + 两次 Enter → 返回 `"D:\\ZCodium\\.plan"` |
| `selectFile` | 取消 → `null`；选择 `D:\ZCodium\AGENTS.md` → 原样返回该路径 |
| `saveFile`（`data`） | 取消 → `{"canceled":true,"success":false}`；写入 `D:\ZCodium\.plan\e2e-save.txt` → `{"path":…,"success":true}`，25 字节且内容一致 |
| `saveFile`（`sourceUrl`） | 成功落盘（19 字节一致）、取消 → `canceled:true`、65MiB 源 → `save_file_source_url_too_large`（不留文件）、挂起源 → 30s 后 `save_file_source_url_timeout`（不留文件） |
| `openInFileManager` | 不存在 → `{"success":false,"error":"path_unavailable: 系统找不到指定的文件。 (os error 2)"}`；目录/文件 → `{"success":true}`，Explorer 标题 `D:\ZCodium` |
| `showTaskNotification` | `(void)`；系统通知可见，标题/正文与入参一致（`.plan/notification-shot.png`） |
| `notifyRendererReady` / `syncWindowTabs` / `syncWindowUnreadCount` | 均 `(void)` |
| `getApplicationIcon` | `null`，未抛错 |
| 深链 | 二次启动带 `mode://oauth/callback?state=test` → 验收台显示 `oauth-callback  mode://oauth/callback?state=test` |

证据：`.plan/uia-after-<index>.txt`、`.plan/uia-run.ps1`、`.plan/uia-buttons.ps1`。
Rust 门禁（`cargo test` 六个 crate + `clippy -D warnings`）同期全绿。

**回滚方式**：跑 smoke 会把本机 `HKCU\Software\Classes\mode` 的 `shell\open\command`
从已安装的 Electron 应用改指到 `target/` 下的调试产物（`app.deep_link().register()` 的
副作用）。本次已用跑前导出的 `reg export` 备份还原（备份留在 `.plan/mode-protocol-backup.reg`）；
再跑时同样先 `reg export` 备份、跑后 `reg import` 还原。

## 5. M1 未完成项（明确登记，不假装完成）

| 项 | 原因 | 归口 | M3 Task 4 的处置 |
| --- | --- | --- | --- |
| **存量 renderer 的挂载** | 启动链要求 Node Host 经 MessagePort 提供 `IServiceAccessor` 与数据库启动通道（§4.2 的证据表）；没有 Host 就只会停在启动/失败屏 | M3（DR-01 sidecar） | **Task 3 关闭** |
| `getApplicationIcon` 的原生图标解码 | 需按平台取 exe / AUMID / bundle 的图标资源并编码为位图数据 URL（Windows GDI、macOS `NSWorkspace`），是一件独立于命令面注册的活 | M3（与 CUA 展示面一起） | **Windows 半边关闭**（§9.2）；**macOS 半边未实现且本阶段不验**（Ruling 20），仍留在本表 |
| `saveFile` 的 `sourceUrl` 分支 | 需要一个 HTTP 客户端与取回超时/大小上限的产品规则 | M3（sidecar 接入后） | **关闭**（§9.3：超时 30s / 上限 64 MiB，均有用例） |
| `onOAuthCallback` / `onPaymentCallback` 之外的订阅型方法 | 依赖 sidecar 的事件源 | M3 | **深链驱动的两条关闭**（`onShareImport` / `onOpenWorkspacePath`，§9.4）；其余**逐条登记**在 §9.4，事件源不在本阶段范围 |
| **深链的单实例转发** | Windows/Linux 上「应用已在运行 → 用户点 `mode://` 链接」会起新进程（新窗口）。单实例会改动窗口生命周期语义，与「每个窗口一个 window-scoped Local Host」绑在一起 | M3（窗口模型） | **关闭**（§9.1：第二实例不建窗口、不建 Host；窗口模型的约束与代价见该节） |
| **Tauri 产物进 Release** | 产物名与旧 Electron 线完全一致，同名会 clobber 掉用户正在下载的安装包；故 M1–M4 只发 workflow artifacts | M5（「release.yml 只出 Tauri 产物」） | **关闭（2026-10-19）**：三平台安装包与校验片段进 Release；旧 Electron 线只发 workflow artifact（§8.1） |
| **LAN / 远控 / 远程工作区的产品壳接线** | `window.mode` 的 LAN/RD 方法仍是拒绝/空操作；`connectRemote` 拒绝；`host-standalone` 未启动 LAN/RD/remote registry；remote scope 缺少 scoped attachment | M3.5（独立立项） | **未关闭（2026-10-19 立项）**：分阶段迁移计划见 `docs/plans/2026-10-19-lan-remote-sidecar-migration.md` |
| **附件多选与本地路径解析（`selectFiles` / `getPathForFile`）** | 壳命令切片只有单选 `selectFile`；`getPathForFile` 需要 WebView2 拖拽路径能力，两者都未实现 | M3.5（附件面） | **未关闭（2026-10-21）**：`desktopPlatform.selectFiles` 在 `window.mode.selectFiles` 缺席时**不得定义**，让 `openAttachmentPicker` 回退到 `selectFile`；定义空实现会让 "+" 按钮拿到空数组并静默无响应。`getPathForFile` 缺席时拖拽/粘贴统一走分片上传（依赖窗口端口可信连接作用域，见 `tauri-renderer-host.md` §11.1） |
| 更新链的实际可用 | **M5 Task 1 关闭（Windows）**：`pubkey` 已从空串换成真实 minisign 公钥，发布构建产出 `latest.json`，产品入口 `executeDesktopCommand("checkForUpdates")` 已接到 `updater::command::check_and_install`。**空串仍是 fail closed**：没有 `pubkey` 的构建在 `PublicKey::decode("")` 处报错，永远无法更新——这一语义刻意保留。macOS/Linux 安装包自 2026-10-19 起同批发布，但 `latest.json` 仍只有 Windows 条目，两条腿的应用内更新未接（§8.1） | M5 |

## 6. 协议双端生成

- Rust 侧 `crates/protocol` 是 `SaveFileRequest` / `SaveFileResult` /
  `TaskNotificationPayload` / `ApplicationIconRequest` 等类型的**真源**，
  带 `#[derive(schemars::JsonSchema, Serialize, Deserialize)]`；
- 导出方向单向：Rust → TS（`packages/shared/src/generated/tauri-protocol.ts`），
  由 `crates/protocol/src/bin/emit-schema.rs`（schemars → JSON Schema）+
  `scripts/generate-tauri-protocol.mjs` 生成，CI 用 `--check` 断言生成物无漂移；
- **生成器的唯一约定**：schemars 把 `Option<T>` 表成 `type: ["T","null"]`，无法区分
  「真的可空」与「`skip_serializing_if` 跳过的 Option」。因此本 crate 的 `Option` 字段
  **一律带 `skip_serializing_if = "Option::is_none"`**，生成器则把可选字段的 `null` 分支去掉，
  产物于是与 `platform.ts` 的手写声明逐字一致。要新增「必须写作 null 的可选字段」时，
  先改这条约定，不要让生成物悄悄变形；
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

## 8. 三平台出包与体积基线（M1 Task 5）

### 8.1 产物去向：三平台安装包一起进 Release（2026-10-19 起）

CI job `tauri`（`.github/workflows/release.yml`）按三平台矩阵出包：

| 腿 | runner | Rust target | bundle | 落点 |
| --- | --- | --- | --- | --- |
| Windows | `windows-latest` | `x86_64-pc-windows-msvc` | `nsis` | `Mode-<版本>-win-x64.exe` |
| macOS | `macos-14` | `aarch64-apple-darwin` | `dmg` | `Mode-<版本>-mac-arm64.dmg` |
| Linux | `ubuntu-24.04` | `x86_64-unknown-linux-gnu` | `appimage` | `Mode-<版本>-linux-x64.AppImage` |

文件名格式与发布契约一致（`AGENTS.md`：`Mode-<版本>-<平台>-<架构>.<扩展名>`），三平台安装包
与各自的 `sha256` 片段都上传同一个 Release。**2026-10-19 变更**（人决定，撤销 DR-M5-2 的
Windows-only 偏差）：v0.0.59 的 CI 已实际产出 macOS arm64 dmg 与 Linux x64 AppImage，构建
风险已收口，因此移除两条腿的 `continue-on-error`——任一腿失败都会让 `tauri.result = failure`，
`checksums` / `publish` 不跑，Release 保持 draft，不会发布缺平台的版本。

**旧 Electron `desktop` job 仍构建但只发 workflow artifact**（DR-M5-1 的回滚能力）：它不进
`publish` 的 `needs`、不写 Release。因此运行页会看到两个 Windows job，但 Release 里只有一个
Windows 安装包（Tauri 的 `Mode-<版本>-win-x64.exe`）。

**应用内更新仍只有 Windows**：`latest.json` 只含 `windows-x86_64` 条目；macOS/Linux 目前是
手动下载安装，发布说明必须写明这一点。它们的签名/公证与运行时验收仍未完成（见 §8.3）。

**版本号由 CI 注入**：`tauri.conf.json` 的 `version` 是占位 `0.0.1`（JSON 不支持注释，
写在这里说明），发布构建用 `tauri build --config '{"version":"<发布版本>"}'` 覆盖。
不把版本写进配置文件是因为它与 Release tag 同源，写在两处必然漂移。

**更新产物（M5 Task 1 实测）**：`tauri.conf.json` 必须设
`bundle.createUpdaterArtifacts: true`，否则 NSIS 目录里没有 `.sig`。设置了
`TAURI_SIGNING_PRIVATE_KEY` 后，`tauri build` 产出
`Mode_<版本>_x64-setup.exe` + `.exe.sig`，但**不产出 `latest.json`**；清单由
`node scripts/tauri-update-manifest.ts` 生成（`version` / `pub_date` /
`platforms["windows-x86_64"]{signature,url}`）。没有私钥时构建在 makensis 之后以非 0 退出：
`A public key has been found, but no private key.`

### 8.2 体积基线（Windows 本机实测）

| 平台 | 体积（**NSIS 安装包字节**，门禁脸） | 状态 |
| --- | --- | --- |
| Windows x64（NSIS，M1 验收台前端） | **7.19 MiB** | M1 本机实测（2026-10-16） |
| Windows x64（NSIS，**真产品 UI + Host sidecar**，预算 **(a) 核心**） | **23.29 MiB** | **M3 Task 5 实测**（2026-10-17；`mode-tauri` 壳 + 内嵌 renderer 产物 + Host bundle + `config/`，**不含**随身 Node 与 externals） |
| Windows x64（NSIS，**产品档总包**） | **51.50 MiB** | **M3 Task 5 实测**（= 上者 + 随包 Node v24.18.1 + 剪枝后的 externals 闭包） |
| Windows x64（NSIS，**预算 (b) = 总包 − 核心**） | **28.22 MiB** | **M3 Task 5 实测**（整包 LZMA 的增量） |
| macOS arm64（dmg） | 见 v0.0.59 CI 的 job summary | 构建已通过（2026-10-19 首次出包）；本机未复测 |
| Linux x64（AppImage） | 见 v0.0.59 CI 的 job summary | 构建已通过（2026-10-19 首次出包）；本机未复测 |

口径与路线图一致：**NSIS 安装包字节数**（`tauri-bundle-report.ts` 算 MiB = 字节 / 1024²，
并写进 job summary 与 artifact 目录里的 `size.txt`）。目标初装 ≤30MB（争取 25MB）——
M1 的 7.19 MiB 是**验收台前端**时代的数，**没有含产品 UI 的 renderer 资产**；本节按 M1 的
「M3 挂载真实 UI 后必须重测并更新本表」补上了 M3 Task 5 的三行。

**门禁结论（M3 Ruling 26 的两份预算，均按本表的口径量）**：**(a) 核心 23.29 MiB ≤ 30MB，达标**；
**(b) 的数值待定**（Task 5 只产出数字与建议，不改门禁，见
`docs/plans/2026-10-16-m3-packaging-size.md` 的 DR-T5-3）。
完整口径、三个包（`core`/`pruned`/`unpruned`）的定义、剪枝的原始/压缩对照与原始输出在
`docs/specs/tauri-renderer-host.md` §13.2/§13.4。

> **口径本身仍待人拍板（M5 Task 3）**：「≤30MB」在路线图 M5 指「核心」，在 M5 计划里
> 被描述成「整包」；当前核心 23.29 MiB、整包 51.50 MiB。M5 没有自行改数，也没有挑一个
> 读法；问题与两种读法逐字登记在 `docs/specs/tauri-renderer-host.md` §13.4.1 与
> `RELEASE.md` §1.5.1（三处必须一起改）。DR-M5-2b 只放开了发布阻断，没有定义口径。

> **一处必须读清的对照**：Task 3 曾报过「核心 54.41 MiB」，那是**未压缩文件字节之和、且不含壳 exe**——
> **不是这张表的口径**。两者并不矛盾：同一个系统在同一时刻，换到门禁真正使用的尺子上是 23.29 MiB。
> 历史上这个数字出过三次口径事故（载荷求和 / 传递闭包 / 不含壳 exe 的文件求和），
> 所以本表的每一行都注明它测在哪张脸上。

### 8.3 macOS / Linux 腿的残余风险（构建已通过，运行时未验收）

2026-10-19 起两条腿已进 Release 并阻断发布。v0.0.59 的 CI 证明**构建链路**可用：

- **Linux**：`webkit2gtk` / `libsoup3` 的 dev 包名与 AppImage 打包（`APPIMAGE_EXTRACT_AND_RUN=1`
  绕过构建容器里的 FUSE）均已通过；**终端用户的 FUSE 环境仍未验证**，发布说明给出
  `--appimage-extract-and-run` 兜底。
- **macOS**：矩阵确认产出 `aarch64-apple-darwin`；**未签名、未公证**，Gatekeeper 的首次打开
  路径只在发布说明里给出操作指引，没有真机验收。

以上是登记在案的**未验收项**，不是「已验证没问题」。升级路径：macOS/Linux 目前手动下载替换，
应用内更新未接（§8.1）。

## 9. M3 Task 4：平台面缺口收口

第 1–6 节描述的是 M1 骨架；本节记的是 **M3 Task 4 关掉 §5 里登记给它那几项**之后的口径变化。
命令名、签名与 `rename` 契约**一字未动**（`crates/platform/tests/commands.rs` 的守卫仍绿），
动的只是实现体与接线。

### 9.1 深链的单实例转发（DR-T4-1）

**问题**：Windows/Linux 上「应用已在运行 → 用户点 `mode://` 链接」一定会**起一个新进程**
（链接在 `argv` 里）。那个进程若走完整启动路径，就会再建一个 Host、再建一个窗口——
也就是第二份 workspace 状态，与 `AGENTS.md` 的「每个窗口一个 window-scoped Local Host、
本地 workspace 共享该 Host」相冲。

**做法**：单实例（`tauri-plugin-single-instance`，在 `main.rs` 里**最先注册**）。第二个进程
把 `argv` 交给已运行实例后**自己退出（码 0）**，不建窗口、不建 Host。

**早退为什么是构造上成立的，而不是靠自觉**：插件把这段逻辑放在**插件 setup 钩子**里，而 Tauri
的顺序是「插件 setup 钩子（`Builder::build` 期间的 `initialize_plugins`）→ 事件循环 Ready →
app 的 `setup` 闭包」。本壳的**窗口与 Host 都只在 app `setup` 里建**，所以钩子里
`process::exit(0)` 时二者都还不存在。证据：
`apps/desktop-tauri/test/singleInstance.test.mts`（静态守卫钉「插件注册在 `.setup(` 之前」
「`spawn_host`/`build_host_window` 只在 setup 里」；opt-in 真机用例钉「第二实例的输出里没有
`host startupId=`」「进程表里 Host 数仍是 1」「深链到达了已运行实例的渲染层」）。

**转发通道复用既有的事件通道**（不另造 IPC）：插件的回调把 `argv` 交给
`deeplink::forward_second_instance_args` → 同一个 `DeepLinkRouter` → 同一个 `mode:deep-link`
事件。也就是说**渲染层侧的通道与 M1 的深链完全一致**：没有新事件名、没有新命令、没有新的
渲染层 IPC 面。第二实例的**参数搬运**（Windows 上是命名互斥体 + `WM_COPYDATA` 到第一个实例的
隐藏窗口）由插件实现，本仓没有自己写进程间通道——「不另造 IPC」指的是**渲染层侧**，
这一点在此写明，免得下游把它读成「一个字节的进程间通信都不许有」。

**代价与边界（写清，不藏）**：

- 单实例**改变了窗口生命周期语义**：从此「应用」= 一个进程。若 M4 的 UI 替换要支持**多窗口**，
  这一层需要重新设计（多窗口下「第二个实例」要变成「把链接交给某个窗口」）。本任务只做
  **「同一个 workspace 不重复起 Host」**，**不重构窗口管理**：转发路径只做「聚焦既有主窗口」
  （否则用户再点一次图标会因为「什么都没发生」而以为应用坏了），不新建也不销毁任何窗口。
- 聚焦窗口的调用点在 `apps/desktop-tauri/src-tauri/src/single_instance.rs`；静态守卫断言该文件
  里**不出现** `build_host_window` / `WebviewWindowBuilder` / `spawn_host`。
- **测试与产品壳共用 app identifier**（`dev.mode.app`）：所以 opt-in 用例在开跑前会先断言本机
  没有其它 mode-tauri 二进制在跑（否则转发会交给那个实例，症状是用例报「A 没收到」而原因在环境）。
- 只做 Windows/Linux 的共用路径（Ruling 20）；macOS 上的等价语义未在本阶段验证。

### 9.2 `getApplicationIcon` 的原生解码（DR-T4-2）

**Windows（已实现）**：`crates/platform/src/icon_windows.rs`：

```text
locator（按顺序试）→ 可执行文件路径 → SHGetFileInfoW(SHGFI_ICON|SHGFI_LARGEICON) 取 HICON
  → GetIconInfo + GetDIBits 取 32bpp 像素 → BGRA→RGBA → PNG → data:image/png;base64,…
```

- **载体是 PNG**（`data:image/png;base64,`）：调用方把它塞进 `<img src>`，所以媒体类型必须是
  浏览器认得的容器；`crates/platform/tests/icon.rs` 解回 base64 并断言 PNG 签名与 IHDR 的宽高。
- **取不到就 `null`，绝不抛**（契约允许 `null` 且调用方已处理）；`darwin-bundle-id` 在 Windows 上
  是**安全跳过**而不是抛错。
- **AUMID 的已知边界**：Windows 没有「给定 AUMID 反查可执行文件」的公开 API，所以本实现把
  AUMID 当**文件名/路径**处理并查 `App Paths`（`HKLM`/`HKCU` 的
  `SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\<名字>.exe`）。取不到仍然是 `null`。
- **一处实测发现（值得下一个读的人知道）**：`GetDIBits` 传 `NULL` 的 DC 在真机上返回 0
  （像素一个都不给，而且 `GetLastError` 也是 0——症状只是「图标取不到」），必须给一个
  `CreateCompatibleDC(NULL)` 出来的内存 DC。
- **macOS 半边未实现且本阶段不验**（Ruling 20：本阶段没有 macOS 构建，写了也验不了）——
  非 Windows 平台上该命令**仍然返回 `null`**，这是契约内的取值，不是错误。

**覆盖分工**：CI（`cargo test -p platform`）跑契约形状——不可解析的 locator → `null`、
macOS-only locator 安全跳过、编码器产出真 PNG；**真实取值是 opt-in 真机跑**
（`MODE_TAURI_ICON_PROBE=1 cargo test -p platform --test icon -- --nocapture real_icon`
对 `%SystemRoot%\System32\notepad.exe` 走完整原生路径），输出入仓见本节的证据登记。

### 9.3 `saveFile` 的 `sourceUrl`：超时与大小上限（DR-T4-3）

**这条路径的形态是「任意远端让应用写一个文件」**，所以上限是产品规则而不是实现细节：

| 项 | 值 | 生效方式 |
| --- | --- | --- |
| 字节上限 | **64 MiB**（`source_url::PRODUCT_MAX_BYTES`） | ① 声明的 `Content-Length` 超限直接失败（不下载）；② **下载过程中**按累计字节判（不给长度/撒谎也吃不穿内存） |
| 墙钟超时 | **30 s**（`source_url::PRODUCT_TIMEOUT`） | `reqwest::Client::timeout`，覆盖连接、首字节与整体传输 |
| scheme | 只收 `http://` / `https://` | 在**发起请求之前**拒绝（`file://` 会把「远端取回」变成「本地任意文件读取 + 写到用户选的路径」，`data:` 也一样） |

- 两者是**公开常量**，`crates/platform/tests/source_url.rs` 的 `the_product_limits_are_pinned`
  钉住数值：改上限必须同时改那条用例与本节。
- **取回失败时不写任何字节**（这句只管**取回**这一段）：命令面只在取回 `Ok` 之后调
  `std::fs::write`，所以「取回失败」与「写了一半」在结构上不可能同时发生；取回失败的结果里
  **不带 `path`**（那个路径上不存在文件）。
- **写盘失败是另一回事（与 Electron 线的取舍一致）**：`std::fs::write` 失败时结果**带 `path` +
  底层 I/O 错误**——路径是用户选的，且那里**可能已经留下半截文件**（磁盘满是典型现场：写到一半
  才失败）。把位置报出来，用户才知道去哪儿看/清理；把 `path` 吞掉会让「写失败了」变成
  「什么都没发生」。这条与上面那句是**两种结局**，不要合并读。
- 错码是稳定契约：`save_file_source_url_unsupported_scheme` / `…_too_large` / `…_timeout` /
  `…_failed`；**细节只进 stderr 诊断行**（`FetchFailure.detail`，含 reqwest 的 `source()` 链——
  Ruling 30：`…_failed` 一个码压着「证书不对 / 连不上 / 协议错」三种现场，不看链就分不开）。
- **顺序取舍**：先让用户选路径、再取回（最坏情况是「选完路径再等 30s」）；反过来先取回再弹
  对话框会让「用户还没决定保存」就先发生一次网络下载。
- TLS：`reqwest` 以 `rustls-no-provider` 档编译（与 `tauri-plugin-updater` 同一档），
  provider 由本模块装一次 `ring`（`install_default` 重复调用安全）。
  **TLS 路径的运行证据**（Ruling 30）：`crates/platform/tests/source_url_tls.rs` 用**自签证书的本地
  TLS 服务**真跑握手——断言失败落在**证书校验**（自签不在系统信任库）、且**不是**「没装
  provider」（服务端用显式 provider，所以进程默认 provider 只可能由产品代码装）、并直接断言
  `CryptoProvider::get_default().is_some()`。
  **未覆盖并登记**：一次**成功**的 HTTPS 往返。本函数的客户端只信任系统根
  （`rustls-platform-verifier`），让本地自签服务被接受只有两条路——给产品代码加「注入信任根」的
  测试钩子（改产品行为）、或把自签根装进系统信任库（Windows 上 `certutil` 装 Root 会弹安全确认，
  无人值守不可接受），两条都不做。成功档由 **opt-in 真网络探针**补
  （`MODE_TAURI_HTTPS_PROBE=1 … --test source_url_tls -- --ignored`，本机实测
  `https://example.com/` → 577 字节）。

### 9.4 订阅型方法的逐条处置

M1 §5 登记的是「`onOAuthCallback` / `onPaymentCallback` 之外的订阅型方法」——本轮**逐条**处置。
判据是「本阶段有没有**可发的事件源**」，而不是「好不好做」：

| 成员 | 处置 | 理由 / 事件源现状 |
| --- | --- | --- |
| `onShareImport` | **关闭**（真实现） | 接口声明就是深链驱动（`mode://share/import?code=…`），而宿主从这个 kind 落地起就投递它——只是没有订阅点（§2 的表） |
| `onOpenWorkspacePath` | **关闭**（真实现） | 同上（「main 进程通过 deep link 直接打开本地工作区目录」），`workspace-open` 一直是投递中的 kind |
| `onOAuthCallback` / `onPaymentCallback` | 已实现（M1） | — |
| `onFocusTab` / `onNewTab` / `onNewTask` / `onOpenWorkspace` | **登记为不做** | 事件源是**壳自己的菜单/托盘/快捷键**（旧线由 Electron main 发）。本阶段壳没有这些入口的**发送方**，也没有窗口模型（多 tab 语义属 M4） |
| `onTaskNotificationClick` | **登记为不做** | 需要「通知被点」的回调链（`tauri-plugin-notification` 的通知点击事件 + 任务 id 回传）。当前只做了「展示通知」（`showTaskNotification`） |
| `onWindowFullscreenChanged` | **登记为不做** | 壳**有**窗口事件的来源（`RunEvent::WindowEvent` + `is_fullscreen()`），但发它需要**新增一个事件名**（协议 + 注入层 + 生成物三处一起动）。本任务的授权是「复用既有通道」，新增通道属窗口模型（M4）的改动。**注意它没有随 §9.5 一起接上**：§9.5 的 chrome 状态里没有 `isFullscreen` 字段（声明如此），所以那个布尔值没有来源 |
| `executeDesktopCommand` / `getDesktopWindowChromeState` / `onDesktopWindowChromeStateChanged` | **实现**（fix round，用户报障） | 见 §9.5：自绘标题栏依赖它们，而注入层从前把它们当「降级空操作 / 缺席」 |
| `onRemoteConnectionLog` / `onRemoteSessionClosed` / `onBotRemoteWorkspaceReconnected` / `onRemoteDesktopSessionEvent` | **独立立项（2026-10-19）** | 事件源在远控链路（attachment 调度、远端传输）；迁移计划见 `docs/plans/2026-10-19-lan-remote-sidecar-migration.md`。在 LR-4 完成前仍保持拒绝/空操作，不伪装成功 |
| `onUpdateReady` | **实现**（2026-10-22 增量） | **派生视图**：壳没有独立的 `update-ready` 事件，`mode:update-state` 的 `kind === "update-downloaded"` 就是「已下载完毕」。语义见下面的「更新就绪的派生」 |
| `onUpdateCheckResult` / `onPostUpdateReleaseNotes` | **登记为不做** | M5 Task 1 起手动检查走 `executeDesktopCommand("checkForUpdates")` → `updater::state::check_for_update`，状态经 `mode:update-state` 推送；但「本次手动检查的结果」仍是**命令返回值**，没有事件源。`onPostUpdateReleaseNotes` 需要「安装后首次启动」的检测，壳尚未产出。两者继续按声明形态降级/拒绝，不伪装成功 |

**更新就绪的派生（`onUpdateReady`，2026-10-22）**：Electron 线由 main 发一次性
`update-ready`，preload 另用 `update-state-changed` 维护「最近一次 ready 版本」缓存，并在订阅时回放。
Tauri 线只复用已有的 `mode:update-state`，不新增事件名；规则与 Electron 的缓存语义逐条对齐：

| 状态事件 | 行为 |
| --- | --- |
| `kind === "update-downloaded"` 且版本与缓存不同 | 记入缓存并通知全部订阅方 |
| 同版本重复的 `update-downloaded` | 不重复通知（避免每次状态重放都弹 toast） |
| 其它任何 `kind`（`idle` / `checking` / `update-available` / `download-progress`） | 清空缓存——取消、跳过或 staging error 后不得再用失效版本撑出「重启以更新」 |
| 订阅时缓存非空 | 立即回放（一次性事件，晚挂载的组件否则永远收不到） |
| 首次订阅 | 用 `getUpdateState` 补一次快照（页面加载可能晚于下载完成）；快照与实时事件用 revision 去重，在途旧快照不得覆盖更新的实时状态 |

两处适配（注入层 `window_mode.ts` 与验收台 `tauri-invoke-shim.ts`）实现同一条状态机；事件名
`mode:update-state` 的真源是 `crates/updater/src/state.rs` 的 `UPDATE_STATE_EVENT`，由
`windowModeInjection.test.mts` 对拍，写错即红（否则症状是「订阅装上了却永远收不到」）。

**「登记为不做」不等于沉默**：这些成员在注入层按**声明返回形态**分成两类——
要读值的（`Promise<T>`，T ≠ void）返回**带方法名的被拒 Promise**，没有要读的值的返回
**留痕空操作**；两类都记进 `window.__MODE_WINDOW_MODE__`，并经 `host_platform_report`
打到壳的 stderr（release 里没有 devtools）。所以「能力缺失」是可观察的，而不是伪装成成功。

**两条承接义务的处置**（DR-T4-4，原文登记在 `docs/specs/tauri-renderer-host.md` §18/§20）：

- **`sequence` 归属**：本任务**没有**引入任何合并的状态流——第二实例转发进的是**深链事件**
  （事件没有序号），`host_startup_state` 的发布者仍然只有壳一个、`sequence` 仍然只有壳一个计数器。
  所以「必须只有一个所有者」的条件**未被触发**，归属继续移交（接手方与条件见 Host 契约 §18 ②、
  §20 第 6 条 —— 条件不变：**谁把 Host 的中间阶段接进壳发布的状态流，谁就必须同时定归属**）。
- **`disk` 摘要**：判据是「有没有消费者」，本 checkout 里**没有**（`state.disk` 零消费方；
  `startupDiskSampler` 只在 Electron 的 `hostDatabaseStartup` 路径上），所以本任务不产出，
  继续移交并把条目**留在 Host 契约 §20 的开放项里**（第 5 条）。

### 9.5 自绘标题栏（窗口控制面）——fix round 的用户报障

**报的是什么**：产品的自绘标题栏**点了没反应**（最小化/最大化不动），而且**系统标题栏还在**
——两条标题栏。成因的**两半都不在渲染层**：

| 半边 | 现场 | 修法 |
| --- | --- | --- |
| 按钮是死的 | 三个按钮发的是 `platform.executeDesktopCommand(DesktopCommandIds.MinimizeWindow / ToggleMaximizeWindow / CloseWindow)`（`packages/ui/src/DesktopWindowControls.tsx` 的调用面）。而注入层把 `executeDesktopCommand` 归在「没有要读的值」那一类**降级空操作**里（它声明返回 `Promise<void>`），`getDesktopWindowChromeState` / `onDesktopWindowChromeStateChanged` 干脆缺席 | 三个成员都接上壳的**窗口控制面**（§下面的表） |
| 系统标题栏还在 | `tauri.conf.json` 里没有任何 `decorations` 设置 → Tauri 窗口是**有边框**的 | 产品窗口 `decorations(false)`（`mode_tauri::window` 的 `build_host_window` 参数） |

**两半必须一起做**：只接线不去掉系统栏 = 两条标题栏；只去掉系统栏不接线 = 一条死栏。
**拖动是第三条，必须一起验**：无边框窗口如果连拖都拖不动，比死按钮更糟。

**成员清单（名字逐字取自声明，不发明）**：

| 成员 | 声明（`IPlatformService` / `globals.d.ts`） | 实现 |
| --- | --- | --- |
| `executeDesktopCommand` | 必选，`(command: DesktopCommandId) => Promise<void>` | 壳侧命令 `executeDesktopCommand`；注入层按 id 原样转发 |
| `getDesktopWindowChromeState` | 可选，`() => Promise<DesktopWindowChromeState>` | 壳侧命令 `getDesktopWindowChromeState`（`isMaximized` 真值） |
| `onDesktopWindowChromeStateChanged` | 可选，`(handler: (state: DesktopWindowChromeState) => void) => () => void` | 注入层组合：窗口 `onResized`（Tauri **内置**事件）→ 去抖 120ms → 重读状态 → 只在 `isMaximized` 变了才回调 |

**命令 id 的处置**：`DesktopCommandIds` 有 27 个 id，窗口级的五个由壳实现
（`closeWindow` / `minimizeWindow` / `toggleMaximizeWindow` / `toggleFullScreen` /
`resetWindowSize`，见 `apps/desktop-tauri/src-tauri/src/window_controls.rs`）。其余 id
**不假装成功**：返回 `{ success: false, error: "desktop_command_unsupported: <id>" }` 并在壳的
stderr 留一行（声明的返回是 `Promise<unknown>`，所以这是形状内的取值；既不抛、也不谎报）。

**拖动为什么不会因为去掉系统栏而失效**（这条是前提，不是巧合）：wry 在 Windows 上无条件打开
WebView2 的非客户区支持（`wry 0.57.0` 的 `src/webview2/mod.rs`：
`settings9.SetIsNonClientRegionSupportEnabled(true)`），于是页面 CSS 里的
`app-region: drag` / `no-drag` 由 WebView2 按非客户区处理——渲染层声明的可拖区域**不需要宿主配合**。
渲染层确实声明了它们（本机实测：3 个 `drag` 区域、14 个 `no-drag` 区域，3 个 drag 都在顶部 48px 内）。

**无边框的判据与例外**：判据是 `sidecar_host::decorations_for_shell_entry(MODE_TAURI_SHELL_ENTRY)`
（**纯函数、在 CI 覆盖的 crate 里、两个方向都有用例**），产品入口（未覆写）→ `decorations(false)`
（无边框）；开发/诊断入口（验收台）与集成测试页 → `decorations(true)`（保留系统栏——那两页没有自绘
标题栏，去掉系统栏会让它们既移不动也关不掉）。入口页与窗口形态用的是**同一个输入**，所以两处判据
不可能分叉；调用点不再自己写判据（**这条正是被写反过的地方**：`!entry_overridden` 直接进
`decorations` 会让产品窗口带系统栏、验收台无边框——用户报的第二个症状就是它的产物）。

**「无边框」怎么验（判据换过一次，把过程记下来）**：

| 观察 | 有没有区分力 | 结论 |
| --- | --- | --- |
| 窗口自己的 `is_decorated()`（harness 打 `HARNESS window decorations_expected=… decorations_actual=…`） | **有**：产品档 `false` / 对照档 `true`（实测） | **这是判据** |
| `outerHeight - innerHeight`（`frameDeltaY`） | **没有**：产品档 0、**有系统标题栏时也是 0**（实测） | **不能当判据**，只在报告里留作排查；它在 WebView2 里量的是 webview 自己的客户区，看不到宿主窗口的非客户区 |

判据写反那次之所以没被抓住，就是因为当时用的正是那条**没有区分力**的次级观察。所以
`windowControls.test.mts` 有两条用例：主用例断言 ①（`is_decorated`）+ ②（可拖区域）+
③（按钮打到壳）+ ④（关得掉）；**敏感性对照**把窗口做成**有**系统标题栏，断言 `is_decorated` 读到
`true`、并**记录**次级观察仍是 0（它变了会红，提示可以升级判据）。

**macOS 半边未实现且本阶段不验**（Ruling 20）：无边框参数是跨平台的，红绿灯（traffic lights）
的占位语义与 `supportsNativeRoundedCorners` 的 macOS 取值都没有 macOS 构建可验。

#### 自动 vs 人工（fix round 的覆盖分工）

| 判据 | 怎么覆盖 |
| --- | --- |
| 无边框（只有一条标题栏） | **自动**：`windowControls.test.mts` 断言窗口自己的 `is_decorated() === false`（产品档）/ `=== true`（敏感性对照档）；`crates/sidecar-host` 的单测 `only_the_product_entry_is_frameless` 钉住判据本身（两个方向，进 CI） |
| 可拖区域存在 | **自动**：探针在 DOM 建好后按 computed style 数 `app-region: drag` 区域（实测 3 个，全在顶部） |
| 按钮打到壳 | **自动**：最大化切换 → 读回 `isMaximized: true` → 切回 → `false`；最小化返回 `success: true`；两层独立证据（注入层的调用 + 壳 stderr 的 `window_control command=… ok=true`） |
| 关得掉 | **自动**：序列最后真发 `closeWindow`，断言进程退出且不残留 Host |
| **用鼠标真的拖动窗口** | **人工**（要真实指针输入，自动化做不到）：发版前手拖一次标题栏。可自动化的必要条件（drag 区域存在 + WebView2 非客户区支持打开）都在上面的自动档里 |
| 图标形状/任务栏状态等视觉结果 | **人工**（同上） |
| 命令名字面量不漂移 | **自动**：`windowModeInjection.test.mts` 的「壳侧窗口控制面的 rename 字面量必须与注入名一致」——`window_controls.rs` 的 `rename` 字面量 ↔ `bridge.rs` 的 `CMD_*` 常量双向对拍（漂移即运行期 `Command not found`） |

### 9.6 本节的证据登记（原始输出）

**单实例**（opt-in 真机；RED 是修复前的现场）：

```text
# RED（修复前）：第二实例起了自己的 Host 与窗口
✖ 第二实例把深链交给已运行实例后退出：不起第二个 Host、不建窗口，链接到达渲染层
  AssertionError: 第二实例**不得**起第二个 Host，实际输出：
  HARNESS host startupId=host-17100-1792159189021 pid=15920
  HARNESS injection=window_mode+bridge startupId=host-17100-1792159189021 pid=15920
  …

# GREEN（修复后）
✔ 单实例接线：插件最先注册、第二实例的转发走既有深链事件通道、且不建窗口 (1.2073ms)
[single-instance] 第二个实例的深链已转入：mode://oauth/callback?state=single-instance-forward
SINGLE_INSTANCE_PAGE_LOG deep_link_received url=mode://oauth/callback?state=single-instance-forward
✔ 第二实例把深链交给已运行实例后退出：不起第二个 Host、不建窗口，链接到达渲染层 (76901.3761ms)
ℹ tests 2   ℹ pass 2   ℹ fail 0
```

**图标（Windows 原生解码，opt-in 真机）**：

```text
$ MODE_TAURI_ICON_PROBE=1 node scripts/rust-cargo.ts test -p platform --test icon -- --nocapture real_icon
[icon-probe] path=C:\Windows\System32\notepad.exe prefix=data:image/png;base64, base64_bytes=3054
test result: ok. 1 passed; 0 failed
```

**TLS 路径**（Ruling 30；自签证书的本地服务，`--nocapture` 的原始输出）：

```text
$ node scripts/rust-cargo.ts test -p platform --test source_url_tls
running 3 tests
test real_https_probe_over_the_public_internet ... ignored, opt-in：要真网络
test the_failure_detail_is_carried_but_the_code_stays_stable ... ok
test a_self_signed_https_endpoint_fails_at_certificate_validation_with_a_provider_installed ... ok
test result: ok. 2 passed; 0 failed; 1 ignored

# 失败细节真身（证明失败落在证书校验、且不是「没装 provider」）：
[tls-probe] kind=Failed detail=error sending request for url (https://127.0.0.1:…/payload)
  <- client error (Connect) <- invalid peer certificate: Other(OtherError(CaUsedAsEndEntity))
  <- invalid peer certificate: Other(OtherError(CaUsedAsEndEntity))

# 成功档（opt-in 真网络；本机实测）：
$ MODE_TAURI_HTTPS_PROBE=1 node scripts/rust-cargo.ts test -p platform --test source_url_tls -- --ignored --nocapture
[https-probe] url=https://example.com/ bytes=577
test result: ok. 1 passed; 0 failed; 0 ignored
```

**自绘标题栏（§9.5；opt-in 真窗口，**按提交的参数**重跑，原始输出）**：

```text
$ cd apps/desktop-tauri && MODE_TAURI_WINDOW_CONTROLS=1 node --import tsx --test test/windowControls.test.mts
HARNESS window decorations_expected=false decorations_actual=false
WINDOW_CONTROLS_PAGE_LOG frame_delta x=0 y=0
WINDOW_CONTROLS_PAGE_LOG drag_regions drag=3 top_drag=3 no_drag=14
WINDOW_CONTROLS_PAGE_LOG window_control chrome_before_toggle {"isMaximized":false}
[mode-tauri] window_control command=toggleMaximizeWindow window=main ok=true at_ms=0 is_maximized=true
WINDOW_CONTROLS_PAGE_LOG window_control chrome_after_toggle_1 {"isMaximized":true}
[mode-tauri] window_control command=toggleMaximizeWindow window=main ok=true at_ms=619 is_maximized=false
WINDOW_CONTROLS_PAGE_LOG window_control chrome_after_toggle_2 {"isMaximized":false}
[mode-tauri] window_control command=minimizeWindow window=main ok=true at_ms=1248 is_maximized=false
WINDOW_CONTROLS_PAGE_LOG window_control minimize_result {"success":true}
[mode-tauri] window_control command=closeWindow window=main ok=true at_ms=1852 is_maximized=false
WINDOW_CONTROLS_PAGE_LOG window_control close_requested
✔ 自绘标题栏在真窗口上可用：无边框、可拖区域在、三个按钮真的打到壳、且关得掉 (41286.0146ms)
# 敏感性对照档（MODE_PRODUCT_UI_FORCE_DECORATIONS=1，见下一条断言）
HARNESS window decorations_expected=true decorations_actual=true
WINDOW_CONTROLS_CONTROL frameDeltaY=0 frameDeltaX=0
✔ 敏感性对照：加系统标题栏时必须看得出来（判据没有区分力就等于没有判据） (72005.5244ms)
ℹ tests 2   ℹ pass 2   ℹ fail 0
```

读法：`decorations_actual=false` 是**窗口自己的状态**（产品档无边框）；对照档 `true` 说明这条观察
真的在读窗口，而**同一档里 `frameDeltaY` 仍是 0** —— 即 `outerHeight - innerHeight` 在 WebView2 下
**看不出**非客户区（它不能当判据，见 §9.5 的表）。`at_ms` 是壳进程内的毫秒数：
`toggleMaximizeWindow` 的往返（`false→true→false`）、`minimizeWindow`、`closeWindow` 都在毫秒级。

**判据写反时的 RED（同一套用例，把调用点的极性翻回去）**：

```text
# ① 单测（进 CI：cargo test -p sidecar-host）
test tests::only_the_product_entry_is_frameless ... FAILED
  panicked at crates\sidecar-host\src\lib.rs:216: 产品入口（未覆写）必须是**无边框**…

# ② opt-in 真机
HARNESS window decorations_expected=true decorations_actual=true
✖ 自绘标题栏在真窗口上可用：… (39755.1055ms)
  AssertionError: 产品入口必须请求**无边框**（decorations=false）：{"expected":"true","actual":"true"}
```

**`sourceUrl` 的上限**（CI 即可跑，真 socket、不碰外网）：

```text
$ node scripts/rust-cargo.ts test -p platform --test source_url
running 6 tests
test the_product_limits_are_pinned ... ok
test a_body_over_the_cap_fails_instead_of_growing ... ok
test a_stalled_response_times_out_as_its_own_failure ... ok
test non_http_schemes_are_rejected_before_any_request ... ok
test a_normal_response_yields_its_bytes ... ok
test the_failure_codes_are_stable ... ok
test result: ok. 6 passed; 0 failed
```

### 9.7 与既有语义的关系（本任务的追加口径）

- **单实例不削弱 owner/lease**：它只保证「同一个 workspace 不重复起 Host」，附件调度与
  owner/lease 语义一个字没动；多窗口下的窗口模型是 M4 的事（§9.1 的代价条款）。
- **命令面契约不动**：`crates/platform/tests/commands.rs` 的四条守卫（切片表、camelCase 映射、
  `rename` 源码扫描、无重名）在本任务后仍然全绿——本任务只换实现体。
- **渲染层零改动**：`git diff -- packages/desktop/src/renderer/**` 为空（硬约束）。

## 10. 独立立项登记：LAN / 远控 Host sidecar 迁移（2026-10-19）

本阶段的命令面不变，但 LAN / 远程桌面 / 远程工作区的**业务面**不再由 `window.mode` 实现。
迁移计划：`docs/plans/2026-10-19-lan-remote-sidecar-migration.md`；
Host 侧契约：`docs/specs/tauri-renderer-host.md` §21。

### 10.1 与 `window.mode` 的关系

- `window.mode` 的 LAN/RD 成员**保持** `REJECTING_REQUIRED` / `NO_OP_REQUIRED` 的现有分类，
  作为回滚 fallback；它们不再是产品能力的实现路径；
- 产品 `IPlatformService`（`packages/desktop/src/renderer/src/desktopPlatform.ts`）在
  Tauri runtime 下把 LAN/RD 请求委托给 Host 服务频道（`lan-access` / `remote-desktop`），
  拿不到 Host 服务时才回退 `window.mode`；
- `removeLanPairedPeer` / `renameLanPairedPeer` 的**静默空操作**是缺陷：Tauri 下必须经
  Host 真实写/删凭据；Electron 下仍走既有 IPC。两者都不允许「看起来成功」；
- 壳只保留 OS 能力（剪贴板读取、显示器枚举/几何、光标位置、同意窗）。Host 不直接读
  系统剪贴板，也不枚举显示器。

### 10.2 不新增 Tauri 命令

LR-1/LR-2 的 LAN 与远程桌面管理面全部走**既有** `MessagePort` 上的 RPC 频道，
不新增 `#[tauri::command]`，也不新增 Rust 事件通道。因此 §1 的切片表与
`crates/sidecar-host/src/bridge.rs` 的命令名集合**不变**；
`windowModeInjection.test.mts` 的声明面/调用面覆盖断言继续成立。

### 10.3 门禁

本节只登记接口关系，不改变本 spec 的验收场景。LR 各阶段的必跑命令与验收项见
`docs/plans/2026-10-19-lan-remote-sidecar-migration.md` §7 与 Host 契约 §21.7。

### 10.4 LR-2 的落地范围（2026-10-19）

`remote-desktop` 频道（local-only）已接上被控设置面：`rdGetServerState`、
`rdSetServerEnabled`、`rdAuthorizeClient`、`rdRevokeClient`、`rdGetPeerCapability`、
`rdGetControlledState` 在 Tauri runtime 下委托 Host，Electron 保持 `window.mode` 路径。

仍未迁移（LR-3）：`rdStartSession`、`rdSendSignal`、`rdEndSession`、`rdReadClipboardText`、
`onRemoteDesktopSessionEvent`。它们在注入层继续按声明返回形态拒绝/空操作，
`desktopPlatform` 也仍原样转发给壳，不被 LR-2 提前接管。

`remote-desktop` 与 `lan-access` 一样只注册在窗口 renderer 的 local-only 集合：
授权列表与管理面不对 LAN 对端开放。

### 10.5 LR-3 的落地范围（2026-10-19）

主控方向已迁到 Host：`rdStartSession`、`rdSendSignal`、`rdEndSession`、
`onRemoteDesktopSessionEvent` 在 Tauri runtime 下委托 `remote-desktop` 频道；
`onDynamicSessionEvent` 只发当前订阅者，不缓冲旧信令。被控同意请求经
`onConsentRequest` / `respondConsent` 暴露给 renderer（60s 超时、已记住设备直通）。

后续阶段补齐（2026-10-19 更新）：
- **被控侧协调器**：已迁移到 Host（`host/remoteDesktopControlledCoordinator.ts`）；
  剪贴板门控仍由媒体中继负责。
- **`rdReadClipboardText`**：已是壳命令（§1 第 13 条，Windows CF_UNICODETEXT）；
  主控剪贴板同步可用。
- **同意窗/指示条**：renderer 模态（`RemoteDesktopConsentDialog` / `RemoteDesktopIndicator`）已落地；
  壳 `WebviewWindow` 形态仍待产品决策（DR-LR-1）。

### 10.6 LR-4 的落地范围（2026-10-19）

`remote` 频道（local-only）已接 LAN 远程工作区：`connectRemote`、`cancelPendingRemoteConnection`、
`bindRemoteWorkspaceSessionContext`、`disposeRemoteSession` 在 Tauri 下委托 Host；
`onRemoteConnectionLog` / `onRemoteSessionClosed` 走 Host 动态事件。

`connectRemote` 成功后会：
1. Host `remote.connect` 建 logical session（LAN 复用既有配对令牌）；
2. `remote.attach` 建 scoped `ChannelServer`；
3. 页面桥 `createScopedServicePort` 投递 `mode:scoped-service-port`，renderer 用既有
   `registerRemoteWorkspaceSession` 注册；
4. renderer 的 `ScopedServicePortReady` 被桥翻译成 `scope-ready` 控制帧，Host 才发 Initialize。

`bindRemoteWorkspaceSessionContext` 会换一个新 scope；旧 scope 由页面桥回收。
SSH/WSL/Docker 在 Tauri Host 明确返回不支持；bot reconnect 与 UI 断线重连降级仍待做。

### 10.7 LR-5：`canOpenCommunity` / `openCommunity` 升级为注入层真实现（2026-10-23）

这两条从 `window.mode` 的 `REJECTING_REQUIRED` 移出，改为**注入层直接实现**
（与 `getInstalledEditors` / `openInEditor` 同一模式，登记在 shim 的
`HOST_INJECTED_IMPLEMENTED_METHODS`——不经 Rust 命令切片，不新增壳命令）：

- **语义依据**：官方帮助配置已整体下线（`isOfficialServiceEnabled("clientConfig")` 恒 false，
  真源 `packages/shared/src/officialPlatformPolicy.ts`），helpAppConfig 读取器对
  zh-CN / en-US / fa-IR 三语一律返回兜底入口 `MODE_ISSUES_URL`，不发起网络请求。
- **`canOpenCommunity(locale)`**：注入层直接 `Promise.resolve(true)`（三语入口恒存在）。
- **`openCommunity()`**：复用壳命令 `openExternal` 打开 `MODE_ISSUES_URL`；
  失败留在 Promise 语义里（`invokeAsync`），不留静默成功。
- 注入层无导入能力，URL 按 shared 真源硬抄，注释标明改址需两处同步；
  验收台（`tauri-invoke-shim.ts`）与注入层同语义，由 `windowModeInjection.test.mts` 对拍。
- `desktopPlatform` 恢复直通委托（`window.mode.canOpenCommunity` / `openCommunity`）。
- `exportLogs` **已从 `REJECTING_REQUIRED` 迁出**（2026-10-26）：日志落盘口径已由 Host
  文件日志（`466dcb0e`）确定，壳命令 `exportLogs` 按命令表第 15 行实现；两处注入面同步
  改为真实现，由 `windowModeInjection.test.mts` 对拍。
