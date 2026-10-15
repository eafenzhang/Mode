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

1. 构建壳：`pnpm --filter @mode/desktop-tauri exec tauri build`（或本机 `cargo run -p mode-tauri`），
   启动后主窗口可见；
2. 前端挂载（§4.2）：窗口里渲染出 M1 命令面验收台，深链订阅已装好并在启动时发出握手；
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

4. 深链通道（M1 唯一接线的事件链）：用 `mode://oauth/callback?state=test` 打开应用，
   验收台的「深链回调」区出现该 URL；
5. `cargo clippy -D warnings` 零告警；`cargo test` 全绿（含 §1 的名字守卫）；
6. 安装包内不含 Node 运行时。

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

## 5. M1 未完成项（明确登记，不假装完成）

| 项 | 原因 | 归口 |
| --- | --- | --- |
| **存量 renderer 的挂载** | 启动链要求 Node Host 经 MessagePort 提供 `IServiceAccessor` 与数据库启动通道（§4.2 的证据表）；没有 Host 就只会停在启动/失败屏 | M3（DR-01 sidecar） |
| `getApplicationIcon` 的原生图标解码 | 需按平台取 exe / AUMID / bundle 的图标资源并编码为位图数据 URL（Windows GDI、macOS `NSWorkspace`），是一件独立于命令面注册的活 | M3（与 CUA 展示面一起） |
| `saveFile` 的 `sourceUrl` 分支 | 需要一个 HTTP 客户端与取回超时/大小上限的产品规则 | M3（sidecar 接入后） |
| `onOAuthCallback` / `onPaymentCallback` 之外的订阅型方法 | 依赖 sidecar 的事件源 | M3 |
| **深链的单实例转发** | Windows/Linux 上「应用已在运行 → 用户点 `mode://` 链接」会起新进程（新窗口）。单实例会改动窗口生命周期语义，与「每个窗口一个 window-scoped Local Host」绑在一起 | M3（窗口模型） |
| **Tauri 产物进 Release** | 产物名与旧 Electron 线完全一致，同名会 clobber 掉用户正在下载的安装包；故 M1–M4 只发 workflow artifacts | M5（「release.yml 只出 Tauri 产物」） |
| 更新链的实际可用 | 只声明了 feed 端点与插件；发布 `latest.json`、签名密钥、三平台更新演练都没做 | M5 |

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

### 8.1 产物去向：workflow artifacts，不进 Release

CI job `tauri`（`.github/workflows/release.yml`）按三平台矩阵出包：

| 腿 | runner | Rust target | bundle | 落点 |
| --- | --- | --- | --- | --- |
| Windows | `windows-latest` | `x86_64-pc-windows-msvc` | `nsis` | `Mode-<版本>-win-x64.exe` |
| macOS | `macos-14` | `aarch64-apple-darwin` | `dmg` | `Mode-<版本>-mac-arm64.dmg` |
| Linux | `ubuntu-24.04` | `x86_64-unknown-linux-gnu` | `appimage` | `Mode-<版本>-linux-x64.AppImage` |

文件名格式与发布契约一致（`AGENTS.md`：`Mode-<版本>-<平台>-<架构>.<扩展名>`），但**只上传为
workflow artifacts**：Tauri 线在 M5 之前不是发布线，而它与旧 Electron 线的产物名逐字相同，
同名上传会 `--clobber` 掉用户正在下载的 Electron 安装包。M5 把上传目标换到 Release 即可，
用户侧的文件名契约不用动。

**版本号由 CI 注入**：`tauri.conf.json` 的 `version` 是占位 `0.0.1`（JSON 不支持注释，
写在这里说明），发布构建用 `tauri build --config '{"version":"<发布版本>"}'` 覆盖。
不把版本写进配置文件是因为它与 Release tag 同源，写在两处必然漂移。

**该 job 不在 `publish` 的 `needs` 里**：三条腿都没在本机验证过（本机只有 Windows + gnu
工具链），把未验证的腿挂成发布门禁会让每次 push 都卡成 draft。转正是 M1 阶段收口时的事，
条件写在下节。

### 8.2 体积基线（Windows 本机实测，2026-10-16）

| 平台 | 体积 | 状态 |
| --- | --- | --- |
| Windows x64（NSIS） | **7.19 MiB** | 本机实测（`tauri build --bundles nsis`，验收台前端） |
| macOS arm64（dmg） | 待首次 CI 记录 | 未测 |
| Linux x64（AppImage） | 待首次 CI 记录 | 未测 |

口径与路线图一致：**NSIS 安装包字节数**（`tauri-bundle-report.ts` 算 MiB = 字节 / 1024²，
并写进 job summary 与 artifact 目录里的 `size.txt`）。目标初装 ≤30MB（争取 25MB），
Windows 实测 7.19 MiB，余量充足——注意这个数字**还没含产品 UI 的 renderer 资产**
（M1 是验收台前端，§4.2），M3 挂载真实 UI 后必须重测并更新本表。

### 8.3 首次 CI 运行要盯的两处

- **Linux**：`webkit2gtk` / `libsoup3` 的 dev 包名随发行版变动；AppImage 的打包器
  （linuxdeploy）依赖 FUSE，容器里靠 `APPIMAGE_EXTRACT_AND_RUN=1` 绕过。若仍失败，
  改用 `--bundles deb`（只需 `dpkg-deb`，风险最低）并同步 matrix 的 `ext` 与 §8.1 的表。
- **macOS**：确认产出的是 `aarch64-apple-darwin`（不是 Rosetta 下的 x86_64）。
