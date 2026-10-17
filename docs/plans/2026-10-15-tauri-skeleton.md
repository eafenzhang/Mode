# M1 Tauri 骨架 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或 `executing-plans`
> 逐任务实现。步骤用 `- [ ]` 复选框跟踪。

**Goal:** 让 Tauri v2 壳跑通存量 React UI 与平台服务层，三平台出包，初装体积出现数量级下降。

**Architecture:** 同仓绞杀者——`apps/desktop-tauri/`（Tauri v2 壳，`src-tauri/`）与 `crates/`（共享 Rust
库：`platform` / `protocol` / `updater` / `tray` / `deeplink`）并存于旧 Electron 栈旁；Rust 侧只做
**平台命令面**，业务仍由既有 Node/TS 侧提供，经 sidecar（M3）接入。协议类型以 schemars + typeshare
从 Rust 结构体单向导出到 TS，CI 断言无漂移。

**Tech Stack:** Rust 1.9x（`rust-toolchain.toml` 已钉 `stable`）、Tauri v2（`tauri-plugin-updater` /
`deep-link` / `os` / `tray`）、tokio、schemars、typeshare、React 18 + TS 5（存量 UI，DR-02 挂载）。

**Spec:** 本阶段 spec 于 Task 1 产出：`docs/specs/tauri-platform-commands.md`。
上游依据：`docs/plans/2026-10-09-tauri-rewrite-roadmap.md` M1 节。
纪律：`docs/specs/clean-room-policy.md`——`crates/**` 与 `apps/desktop-tauri/**` 全部适用；
`packages/shared/src/platform.ts` 属上游衍生文件，**只允许读接口面（签名/类型声明）**，禁读实现体。

## Global Constraints

- 语言政策：新增只允许 Rust / TS / HTML / CSS；Tauri 工程不得引入 `.cs`/`.py`。
- `IPlatformService` 是命令面的**接口规格**：方法与参数逐名对齐，不做重命名。
- 三平台（Windows x64 / macOS arm64 / Linux x64）全程 CI 出包；体积口径按 NSIS 安装包字节数。
- 既有语义不得丢失：owner/lease、CommandInbox 串行准入、`desktop-continuous` 与
  `web-remote-replayable` 双链路、workspaceIdentity 贯穿（本阶段只对命令面打底，不实现运行时）。
- 不带任何 Node 运行时进安装包。
- `electron-builder.config.js` 同一窗口期只允许一处改动（CUA 一期与 browser-import 重写已各占一次）。

---

### Task 1: 阶段 spec + cargo workspace + 脚手架 + CI

**Files:**
- Create: `docs/specs/tauri-platform-commands.md`（阶段 spec）
- Create: `Cargo.toml`（workspace 根；成员见 Step 1 的显式清单与排除理由）
- Create: `crates/platform/{Cargo.toml,src/lib.rs}`、`crates/protocol/{Cargo.toml,src/lib.rs}`
- Create: `apps/desktop-tauri/src-tauri/{Cargo.toml,build.rs,tauri.conf.json}`
- Create: `apps/desktop-tauri/src-tauri/src/main.rs`
- Modify: `pnpm-workspace.yaml`（纳入 `apps/desktop-tauri`）
- Modify: `.github/workflows/release.yml`（新增 `tauri-skeleton` 检查 job：`cargo test` + `cargo clippy -D warnings`）

**Interfaces:**
- Consumes: 根 `rust-toolchain.toml`（M0 任务 6 半落地，`channel = "stable"`）。
- Produces: workspace 根 `Cargo.toml`；`platform::AppState`；`ModeTauri` 可执行；CI job 名 `tauri-skeleton`。

> **workspace 成员口径（2026-10-15 决定）**：成员**显式列举**（新 crate + `apps/desktop-tauri/src-tauri`），
> 并 **`exclude = ["crates/mode-cua-ax"]`**，不用 `crates/*` 通配。理由（已核实的耦合面）：
> 1. 加入 workspace 会把 target 目录上移到仓库根，而 `scripts/build-cua-helper.mjs:23-26`
>    期望 `crates/mode-cua-ax/target/<triple>/release/mode_cua_ax.dll` —— 现役已发布 addon 的构建会直接断；
> 2. workspace 会用根锁文件重解析 `mode-cua-ax` 的依赖图，`windows`/`napi` 版本可能被动漂移，
>    这对一个已上线的原生 addon 不是可以顺手做的改动。
> **`crates/mode-cua-ax` 并入 workspace 是独立任务**（要连带改 `build-cua-helper.mjs` 路径、CI
> `rust-cache` 的 `workspaces`、删除成员锁文件），完成后才能把 `exclude` 去掉。

**Stage spec（`docs/specs/tauri-platform-commands.md`）必须包含的四节**（照抄下表，勿留空）：

1. **命令切片**——M1 只实现下列 10 个 `#[tauri::command]`（方法名与 `IPlatformService` 逐字对齐）：

   | # | 方法 | 签名（来自接口面） |
   | --- | --- | --- |
   | 1 | `openExternal` | `(url: string) -> void` |
   | 2 | `selectDirectory` | `() -> Promise<string \| null>` |
   | 3 | `selectFile` | `() -> Promise<string \| null>` |
   | 4 | `saveFile` | `(payload: SaveFileRequest) -> Promise<SaveFileResult>` |
   | 5 | `openInFileManager` | `(path: string) -> Promise<{success, error?}>` |
   | 6 | `showTaskNotification` | `(payload: TaskNotificationPayload) -> void` |
   | 7 | `notifyRendererReady` | `() -> void` |
   | 8 | `syncWindowTabs` | `(paths: string[]) -> void` |
   | 9 | `syncWindowUnreadCount` | `(count: number) -> void` |
   | 10 | `getApplicationIcon` | `(req: ApplicationIconRequest) -> IconInfo \| null` |

2. **事件通道**——订阅型方法（`onFocusTab` / `onNewTab` / `onOAuthCallback` …）在 M1 只落
   **一个**示范：`onOAuthCallback` 走 Tauri event + deep-link（Task 7 接线），其余留 M3。
3. **capability 规则**——`src-tauri/capabilities/default.json` 只开本切片所需权限，逐条列白名单。
4. **验收场景**——「开窗 → 渲染 UI → 10 命令全绿」的 E2E 步骤与失败判据。

- [x] **Step 1: 写 workspace 根与两个 crate 骨架**（2026-10-16 落地；实际建了计划列出的全部 crate。
  另外发现并处理了两个 workspace 化带来的连带项，见文末「2026-10-16 执行记录」）

**已落地**（2026-10-16），内容与下面草稿一致：

```toml
# Cargo.toml
[workspace]
resolver = "2"
members = [
  "crates/browser-import-helper",
  "crates/platform",
  "crates/protocol",
  "crates/updater",
  "crates/tray",
  "crates/deeplink",
  "apps/desktop-tauri/src-tauri",
]
# 见上「workspace 成员口径」：现役 CUA addon 暂不入 workspace，避免 target 目录上移
# 与锁文件重解析打断它的构建；并入是独立任务。
exclude = ["crates/mode-cua-ax"]

[workspace.package]
edition = "2021"
version = "0.1.0"
```

- [x] **Step 2: 跑 workspace 解析**（2026-10-16 通过）

Run: `cargo metadata --no-deps --format-version 1 > /dev/null && echo OK`
Expected: `OK`（注意：`crates/mode-cua-ax` 与 `crates/browser-import-helper` 会一并成为成员，
若它们的 `[package]` 与 workspace 冲突，改在其 Cargo.toml 显式声明而非改根）

- [x] **Step 3: 骨架 main + 空窗口**（2026-10-16 落地；`main.rs` 调 `platform::commands::init`，
  占位前端在 `apps/desktop-tauri/dist/index.html`，Task 4 换成 renderer 产物）

```rust
// apps/desktop-tauri/src-tauri/src/main.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [x] **Step 4: 本机构建**（2026-10-16 实测通过）

Run: `cargo +stable-x86_64-pc-windows-gnu build -p mode-tauri --target x86_64-pc-windows-gnu`
Expected: 成功（首次拉取 crates.io 依赖，耗时较长）——**已通过**，产出 `ModeTauri.exe`。

本机 gnu 路线的工具链装配（llvm-mingw linker + `+stable-x86_64-pc-windows-gnu`）
已由 `scripts/build-browser-import-helper.mjs` 固化，同款 env 可直接复用；
计划原文「本机无 Rust 链接器」的假设已作废（见 browser-import 计划 Global Constraints）。

- [x] **Step 5: CI job（`tauri-skeleton`）与提交**（2026-10-16）

**Deviation**：没有新开独立 job，而是把 `Tauri skeleton tests and clippy` 作为**步骤**挂进
`desktop` job 的 Windows 腿——与既有的 CUA helper / browser-import helper 两步同形。
理由：`desktop` job 的矩阵当前只有 Windows x64 一条腿（mac/linux 已从发版矩阵移除），
再开一个 job 会重复检出与 pnpm 安装；且同形步骤便于 rust-cache 复用同一个 workspace 条目。
CI 里同时把 rust-cache 的 `workspaces` 换成 `crates/mode-cua-ax` + `.`（根 workspace）。

```bash
git add Cargo.toml crates/platform crates/protocol apps/desktop-tauri docs/specs/tauri-platform-commands.md pnpm-workspace.yaml .github/workflows/release.yml
git commit -m "feat(tauri): M1 骨架——workspace 根、平台/协议 crate、CI job"
```

---

### Task 2: 平台命令面（10 个，TDD）

**Files:**
- Create: `crates/platform/src/commands/{fs_dialog.rs,shell.rs,notify.rs,window.rs,icon.rs}`
- Create: `crates/platform/src/commands/mod.rs`
- Test: `crates/platform/tests/commands.rs`

**Interfaces:**
- Consumes: Task 1 的 `platform::AppState`。
- Produces: `platform::commands::init(builder: tauri::Builder) -> tauri::Builder`，
  注册 Task 1 表格里的 10 个命令名。

- [x] **Step 1: 写失败的测试**（对纯粹可测的部分：命令名注册表、名字映射守卫）

```rust
// crates/platform/tests/commands.rs
#[test]
fn registers_exactly_the_m1_slice() {
    let names = platform::commands::registered_names();
    assert_eq!(names.len(), 10);
    for n in ["openExternal", "selectDirectory", "selectFile", "saveFile",
              "openInFileManager", "showTaskNotification", "notifyRendererReady",
              "syncWindowTabs", "syncWindowUnreadCount", "getApplicationIcon"] {
        assert!(names.contains(&n), "missing command {n}");
    }
}
```

- [x] **Step 2: 跑测试确认失败**（编译期 `unresolved import platform::commands`）
- [x] **Step 3: 逐个实现**（2026-10-16 落地）
  切片表由 `commands/mod.rs` 的 `command_slice!` 宏声明为唯一真源（命令名 ↔ 处理函数成对），
  `init()` 与 `registered_names()` 都从它派生。**Deviation**：没有为每条命令都写 `tauri::test`
  级用例——`tauri::test` 需要一个 webview mock 运行时，属于骨架期之外的投入；本步改为
  「切片表 + 名字映射守卫」自动测，命令的真人核对列进 spec §4 的验收表。
  `getApplicationIcon` 与 `saveFile` 的 `sourceUrl` 分支按 spec §5 登记为部分实现。
- [x] **Step 4: 跑测试确认通过**（2026-10-16：3 passed；clippy `-p platform -p protocol -p mode-tauri --all-targets -- -D warnings` 零告警）
  Run: `cargo test -p platform && cargo clippy -p platform -- -D warnings`
- [x] **Step 5: 提交**（2026-10-16）

---

### Task 3: 协议双端（schemars + typeshare）

**Files:**
- Modify: `crates/protocol/src/lib.rs`
- Create: `packages/shared/src/generated/tauri-protocol.ts`（生成物，提交）
- Create: `scripts/generate-tauri-protocol.mjs`
- Test: `crates/protocol/tests/roundtrip.rs`

**Interfaces:**
- Produces: `protocol::SaveFileRequest` / `SaveFileResult` / `TaskNotificationPayload` /
  `ApplicationIconRequest`（字段与 `packages/shared/src/platform.ts` 同名同义）；
  生成脚本输出 TS 类型 + CI 漂移断言。

- [x] **Step 1: 写往返测试**（2026-10-16 落地 `crates/protocol/tests/roundtrip.rs`，6 用例全绿）
  钉住的是**线协议**形状：字段名 camelCase、`Option` 字段「缺席而非 null」、判别联合的
  `kind` 取值、`SaveFileRequest` 的联合不变量。（serde 序列化与 TS 侧同名 JSON 互认）〔已做〕
- [x] **Step 2: 实现结构体 + `#[derive(JsonSchema, Serialize, Deserialize)]`**（2026-10-16 落地 `crates/protocol/src/lib.rs`：`TaskNotificationPayload`/`TaskNotificationStatus`/`ApplicationIconLocator`/`ApplicationIconRequest`/`ApplicationIconInfo`/`SaveFileRequest`/`SaveFileResult`/`OpenPathResult`，全部 `rename_all = "camelCase"`，`SaveFileRequest::validate()` 落实 TS 的「data 与 sourceUrl 恰有其一」不变量）
- [x] **Step 3: 生成 TS 并断言无漂移**（2026-10-16 落地）
  **Deviation**：没用 typeshare，改走 schemars + 仓库内生成脚本——typeshare 要在开发机与 CI
  额外装一个 CLI，而 Rust 侧本来就要 derive `JsonSchema`。实现为：
  `crates/protocol/src/bin/emit-schema.rs`（打印 8 个类型的 JSON Schema）→
  `scripts/generate-tauri-protocol.mjs`（转 TS，写 `packages/shared/src/generated/tauri-protocol.ts`）。
  生成器有一条必须知道的约定：schemars 把 `Option<T>` 表成 `type: ["T","null"]`，无法区分
  「真的可空」与「`skip_serializing_if` 跳过的 Option」，故**可选字段一律去掉 null 分支**，
  产物因此与 `platform.ts` 的手写声明逐字一致。这条约定写在生成器注释里。

Run: `node scripts/generate-tauri-protocol.mjs --check`
Expected: 首次生成后写入文件；再次运行无 diff

- [x] **Step 4: 提交**（2026-10-16）

---

### Task 4: UI 挂载（DR-02）

**Files:**
- Modify: `apps/desktop-tauri/src-tauri/tauri.conf.json`（`build.frontendDist` 指向 renderer 产物）
- Create: `packages/ui/src/platform/tauri-invoke-shim.ts`（`window.mode` → `invoke()` 适配层）

**Interfaces:**
- Consumes: 存量 renderer 产物；`IPlatformService` 的 10 个方法。
- Produces: `createTauriPlatformService(): Partial<IPlatformService>`（**只实现 10 个**，
  其余方法显式抛 `not_implemented_until_m3`，不得静默返回假值）。

- [x] **Step 1: 写失败的测试**（2026-10-16 落地 `packages/ui/test/tauriInvokeShim.test.ts`，8 用例全绿）
- [x] **Step 2a: 实现 shim**（2026-10-16 落地 `packages/ui/src/platform/tauri-invoke-shim.ts`）
  **Deviation（重要）**：原计划「未实现方法一律抛错」被拆成了按接口可选性分流——
  `IPlatformService` 里带 `?` 的方法**保持缺席**（调用方用 `?.` 守卫，塞抛错桩会把
  「平台不支持就跳过」变成「直接崩」），只有**必选**方法才装 `not_implemented_until_m3` 抛错桩
  （59 条）。覆盖关系由类型层面的穷尽检查 `_allRequiredMembersCovered` 在编译期守住：
  协议新增必选方法而 shim 没跟上时 `pnpm typecheck` 直接报错。
- [x] **Step 2b: 挂载前端**〔2026-10-16 落地，**Deviation**〕
  **原计划的「frontendDist 指向 renderer 产物」实测做不到**：存量 renderer 的启动链不只需要
  `window.mode`，还要一个 Node Host 经 MessagePort 提供 `IServiceAccessor` 与数据库启动通道
  （`DatabaseStartupAdmission` 30s 就绪等待），那正是 M3 的 sidecar（DR-01）；没有 Host 只会
  停在启动/失败屏，把它当「渲染 UI」交付是假的。证据表在
  `docs/specs/tauri-platform-commands.md` §4.2。
  实际落地：`apps/desktop-tauri/frontend/` 改成 Vite + TS 的**命令面验收台**——渲染 10 条命令的
  触发入口与结果、深链回调面板、启动即握手。`frontendDist` 指向构建产物 `../dist`，
  `beforeBuildCommand` 跑前端构建（产物不入库）。命令表用 `satisfies` + 编译期守卫保证
  覆盖切片表每一条。
- [x] **Step 3: 本机跑 E2E 场景**〔2026-10-17 执行；完整记录见文末「第四批」〕：开窗 → 渲染 →
  依次触发 10 个命令并核对。步骤与判据见 spec §4。启动会临时改指 `mode://` 注册，
  跑前备份、跑后已用 `.plan/mode-protocol-backup.reg` 还原。
- [x] **Step 4: 提交**（2026-10-16，shim 部分）

---

### Task 5: 三平台出包与体积基线

**Files:**
- Modify: `.github/workflows/release.yml`（`tauri build` job，产物名 `Mode-<版本>-<平台>-<架构>.<ext>`）
- Create: `docs/specs/tauri-platform-commands.md` 附录（三平台体积记录）

- [x] **Step 1: 加 `tauri build` job**〔2026-10-16〕三平台矩阵（win/msvc+nsis、mac-14/arm64+dmg、
  ubuntu-24.04/x64+appimage），产物名按发布契约 `Mode-<版本>-<平台>-<架构>.<ext>`。
  **两处与计划原文不同，都已写进 workflow 注释与 spec §8**：
  (a) 产物**只发 workflow artifacts，不进 Release**——Tauri 与旧 Electron 线的产物名逐字相同，
  同名 `--clobber` 会覆盖用户正在下载的安装包；M5 换上传目标即可，用户侧契约不变。
  (b) 该 job **不挂进 `publish` 的 needs**——三条腿都没在本机验证过，挂上去会让每次 push 都卡成 draft；
  转正条件是首次 CI 三腿全绿。
- [x] **Step 2: 记录体积**〔2026-10-16〕Windows NSIS **7.19 MiB**（本机实测，CI 同款命令）；
  mac/linux 待首次 CI 记录。落点：`scripts/tauri-bundle-report.ts` 写 job summary 与
  artifact 目录的 `size.txt`，基线表在 spec §8.2。注意该数字**尚未含产品 UI 资产**（本阶段是
  验收台前端），M3 挂载真实 UI 后必须重测。
- [x] **Step 3: 提交**〔2026-10-16〕

---

### Task 6: 更新链 stub

**Files:**
- Create: `crates/updater/src/lib.rs`、`crates/deeplink/src/lib.rs`、`crates/tray/src/lib.rs`

- [x] **Step 1: `tauri-plugin-updater` 接现有 GitHub Releases 清单格式**〔2026-10-16〕
  与旧线共用 feed（`eafenzhang/Mode`），只声明 `latest.json` 端点 + 注册插件，**不注册任何
  更新命令**；**不设 pubkey**，让 `check()` 明确失败而不是静默放行未签名更新。
- [x] **Step 2: deep-link 与 tray 落最小可用面**〔2026-10-16〕
  - `crates/deeplink`：解析（行为规格取自 `packages/desktop/tests/deep-link-scheme-compat.test.mjs`
    的断言，黑盒）+ 冷启动缓存路由（`DeepLinkSink` 抽象让时序可单测）+ Tauri 事件投递。
    注册只注册 `mode`，旧 `zcode://` 只在处理侧兼容。
  - `crates/tray`：显示主窗口/退出两项；build 失败降级成 stderr，不阻断启动。
  - `onOAuthCallback` / `onPaymentCallback` 端到端打通：`crates/protocol` 出新的事件名与载荷
    类型（跨端契约真源，生成器新增 `$constants` 段，`--check` 从此也兜住事件名写错），
    shim 用生成的常量订阅、按 `kind` 分流。**握手顺序是语义**：`notifyRendererReady`
    先等订阅装好再发，否则宿主补发的冷启动深链没人接（有专门用例钉住）。
- [x] **Step 3: 提交**〔2026-10-16〕

---

## 阶段验收

**当前状态（2026-10-16 第二批后）**：第 3、5 项已由自动化断言守住；第 4 项由架构保证
（前端是静态资源，无 Node 运行时）；第 1 项只差 macOS/Linux 两条腿的首次 CI；
第 2 项只差人工核对（见「仍未做」）。

1. 三平台安装包全绿且 Windows ≤30MB（争取 25MB）；Windows 实测 **7.19 MiB**（未含产品 UI 资产）
2. Tauri 版完成「开窗 → 渲染 UI → 走通 10 个平台命令」E2E；
3. `cargo clippy -D warnings` 零告警；`cargo test` 全绿；
4. 安装包内不含 Node 运行时；
5. 协议生成物无漂移（CI 断言）。

## 风险

| 风险 | 对冲 |
| --- | --- |
| workspace 化改动波及已上线的 `crates/mode-cua-ax`（CI 用 `--manifest-path` + rust-cache `workspaces`） | Task 1 Step 2 先跑 `cargo metadata`；CI 的两个 cargo job 同步改路径后再合并 |
| 存量 renderer 产物依赖 Electron 专有 API | Task 4 的 shim 显式抛 `not_implemented_until_m3`，不用假值掩盖 |
| typeshare 生成的 TS 与既有 `platform.ts` 语义漂移 | Task 3 往返测试 + CI `--check` 双保险 |
| 体积目标 25MB 受 renderer 资产（53MB 解包口径）牵制 | Task 5 先记基线，超预算时上报而不静默放宽 |

---

## 2026-10-16 执行记录

### 已完成

- **Task 1 全部步骤**：workspace 根 `Cargo.toml`、`crates/{platform,protocol,updater,tray,deeplink}`、
  `apps/desktop-tauri/src-tauri`（含 `tauri.conf.json` / capabilities / 图标 / 占位前端）、
  阶段 spec `docs/specs/tauri-platform-commands.md`、`pnpm-workspace.yaml` 纳入壳目录、
  CI 步骤接入。
- **Task 2 的代码与自动化测试**：10 条命令全部注册并可调用（3 个用例全绿，clippy 零告警）。
  spec §5 明确登记了 `getApplicationIcon` 与 `saveFile(sourceUrl)` 两处**部分实现**。
- **Task 3 全部步骤**：协议类型真源（`crates/protocol`）、往返测试、TS 生成脚本与漂移断言
  （`node scripts/generate-tauri-protocol.mjs --check`，已接进 CI 的 Tauri skeleton 步骤）。
- **Task 4 的适配层部分**：`packages/ui/src/platform/tauri-invoke-shim.ts` + 8 个用例。

### workspace 化的两个连带项（计划原文没列，必须知道）

1. **target 目录上移**：成员 crate 的产物从 `crates/<c>/target/...` 移到仓库根 `target/...`。
   计划里只为 `crates/mode-cua-ax` 点了这条风险（所以它被 `exclude`），但同样命中
   `crates/browser-import-helper`：`scripts/browser-import-helper-oracle.mjs` 的
   `resolveRustHelperPath()` 与 `scripts/build-browser-import-helper.mjs` 的产物路径都已改为
   根 `target/`（并支持 `CARGO_TARGET_DIR` 覆盖），根 `.gitignore` 增加 `/target`，
   成员锁文件 `crates/browser-import-helper/Cargo.lock` 删除（workspace 用根锁文件）。
2. **comctl32 v6 清单**：`cargo test -p platform` 的二进制会被链入 tao 的 Windows GUI 代码，
   而测试二进制不带应用清单，直接 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)`。
   修法是 `crates/platform/build.rs` 只对**测试目标**注入清单（`rustc-link-arg-tests`，
   MSVC 走 link.exe 的 `/MANIFESTINPUT`，gnu 走 windres）。
   详细因果见该文件与 `docs/specs/tauri-platform-commands.md` §4.1——**不要用 tauri-winres**，
   它会把资源库传播给依赖方并与 app 自己的清单撞车。

### 与原计划的口径差异（都已在正文标注）

| 项 | 计划 | 实际 | 原因 |
| --- | --- | --- | --- |
| CI 接线 | 新开 `tauri-skeleton` job | 挂进 `desktop` job 的 Windows 腿 | 该 job 矩阵当前只有 Windows 一条腿，单开 job 要重复检出与安装 |
| 命令级测试 | 每命令一个 `tauri::test` 用例 | 切片表 + 名字映射守卫自动测，其余人工核对 | `tauri::test` 需要 webview mock 运行时，超出骨架期投入 |
| capabilities | 列 6 条插件权限 | 只留 `core:default` | 本切片的插件调用都在 Rust 侧，不经渲染层 IPC，`allow-*` 没有调用方（实证：列了会直接报 permission not found） |
| workspace 成员 | 只提醒 mode-cua-ax 的风险 | 另处理 browser-import-helper 的 target/lock 连带项 | 同一机制的必然结果，计划漏列 |

### 仍未做（截至 2026-10-16 第二批）

- **Task 4 Step 3（E2E 人工核对）**：只有这一项，且必须是真人看窗口/对话框。
  判据表见 `docs/specs/tauri-platform-commands.md` §4。

---

## 2026-10-16 执行记录（第二批：Task 4 Step 2b/3、Task 5、Task 6）

### 已完成

- **Task 6 全部步骤**：`crates/deeplink`（解析 + 冷启动缓存 + 事件投递）、`crates/tray`、
  `crates/updater` 三块最小面，以及 `onOAuthCallback` / `onPaymentCallback` 的端到端接线。
- **Task 4 Step 2b（Deviation）**：`apps/desktop-tauri/frontend/` 变成 Vite + TS 的命令面
  验收台，`frontendDist` 指向构建产物、`beforeBuildCommand` 负责构建。
- **Task 5 Step 1/2**：三平台 `tauri build` job + 体积记录脚本；Windows NSIS 实测 7.19 MiB。

### 第二批发现的连带项

1. **comctl32 v6 测试清单不再只影响 platform**：`crates/deeplink` 一旦依赖 tauri，它的
   集成测试目标同样会 `STATUS_ENTRYPOINT_NOT_FOUND`。故把 `platform/build.rs` 的那套逻辑
   抽成构建依赖 **`crates/tauri-test-manifest`**（各 crate 的 `build.rs` 只剩一行调用）——
   M2/M3 还会加依赖 tauri 的 crate，抄多份等于把同一个坑复制多份。
2. **跨端契约的第三种形态：常量**。深链事件名（`mode:deep-link`）与 `kind` 取值必须两端逐字
   一致，写错是**静默失效**（订阅装上了却永远收不到）。做法是把它和协议类型一起放进
   `crates/protocol`，生成器新增 `$constants` 段导出，`--check` 从此也兜住这类错误。
3. **`@mode/shared` 的生成物不能整模块 `export *`**：它与 `platform.ts` 的手写声明同名
   （`SaveFileRequest` / `TaskNotificationPayload` / `ApplicationIconInfo` …）。故
   `packages/shared/src/index.ts` 只再导出**没有手写对应物**的符号（深链三个）。
4. **根 `Cargo.lock` 上个提交漏了**：workspace 建了但锁文件没入库，CI 无法复现依赖图。
   本批补提交。
5. **`frontendDist` 从「提交进仓库的占位页」改成「构建产物」**：占位页当时是为了躲避根
   `.gitignore` 的 `dist/`，现在改由 `beforeBuildCommand` 现做，产物落在被忽略的 `dist/`
   才是对的（不再需要把构建产物入库）。

### 与计划原文的口径差异（第二批）

| 项 | 计划 | 实际 | 原因 |
| --- | --- | --- | --- |
| Task 4 Step 2b | `frontendDist` 指向 renderer 产物 | 交付命令面验收台，renderer 挂载移到 M3 | renderer 启动链依赖 Node Host（M3 sidecar），没有 Host 只会停在失败屏；证据表见 spec §4.2 |
| Task 5 产物去向 | 与旧线共用 Release | 只发 workflow artifacts | 产物名与旧线逐字相同，同名上传会 clobber 用户的安装包；M5 换目标 |
| Task 5 门禁 | — | 不挂 `publish.needs` | 三条腿未本机验证，挂上去会让每次 push 卡成 draft |
| Task 5 体积表 | 三平台都记 | 只记了 Windows | 本机只有 Windows + gnu 工具链；mac/linux 待首次 CI |

## 2026-10-16 执行记录（第三批：深链 smoke，**修正第二批次的两处结论**）

用户授权后补跑 Task 6 Step 2 的实机深链验证。**结论：自动化全绿的状态下，壳其实一个命令都
调不通、深链一条都收不到。** 三个缺陷已修，明细见 `docs/specs/tauri-platform-commands.md` §4.3。

| # | 症状 | 根因 | 修法 |
| --- | --- | --- | --- |
| 1 | 壳启动即 panic | `plugins.updater` 段缺席；`Config.pubkey` 是必填字段 | conf 补端点 + `pubkey: ""`（空串在 `PublicKey::decode` 处 fail closed），加两条 conf↔常量守卫 |
| 2 | 10 条命令全部 `Command X not found` | `#[tauri::command]` 默认注册 snake_case 名，切片表的 camelCase 名一个都命不中 | 10 个处理函数逐个补 `rename = "<camelCase>"`，加扫源码的 wire-name 守卫 |
| 3 | 深链收不到（面板恒 `(尚无回调)`） | **#2 的症状**：握手命令也命不中，宿主收不到 `notifyRendererReady`，冷启动队列永不回放 | 随 #2 一并解决 |

**要更正第二批次的两处表述**：

1. 第二批写「Task 6 Step 2 已通过」——**当时只证明了编译通过**，没有任何一步真的启动过窗口。
   「已通过」应当读作「编译通过 + 单测通过」，运行期从未验证。
2. 第二批把 `cargo build` 的成功当成了「骨架可用」的证据。实际证据链要包含**启动一次**：
   本次 smoke 的入口就是直接跑二进制并把 stderr 重定向到文件（release 构建是
   `windows_subsystem = "windows"`，没有控制台，但继承来的重定向句柄仍可写）。

顺带记录一条本地环境事实（写进 spec §4 步骤 1 了）：本机直接调 cargo 必须自己装配
gnu 工具链（`+stable-x86_64-pc-windows-gnu` + llvm-mingw 作 linker），否则会以
`dlltool.exe: program not found` 或 `link: extra operand` 失败——本机没装 VS Build Tools，
而 Git Bash 的 coreutils `link` 会挡住 MSVC 的 `link.exe`。

**仍未做**：对话框/文件选择/系统通知这几项需要真人看窗口（无人值守跑不了）；
macOS / Linux 出包腿按用户决定暂不处理。

## 2026-10-17 执行记录（第四批：Task 4 Step 3 的 M1 E2E 核对）

**做法**：重建壳（`node scripts/rust-cargo.ts build --release -p mode-tauri`，1m17s）+
验收台（`pnpm --filter @mode/desktop-tauri run build:frontend` → `dist/dev/`），以
`MODE_TAURI_SHELL_ENTRY=dev/index.html` 启动。WebView2 仍接不上 CDP，沿用 §4.3 的
UIA `TextPattern` 读数方案；本次补了 `.plan/uia-run.ps1`（按索引点按钮、设输入、取消/确认对话框）
与 `.plan/uia-buttons.ps1`（枚举控件）。证据：`.plan/uia-after-<index>.txt`、`.plan/notification-shot.png`。

| 命令 | 结果 |
| --- | --- |
| `openExternal` | `(void)`；Edge 打开 `example.test`（窗口标题证据） |
| `selectDirectory` | 取消 → `null`；**选择分支未验**（旧式文件夹选择框不接受键入路径，Esc 后仍为 `null`） |
| `selectFile` | 取消 → `null`；选择 `D:\ZCodium\AGENTS.md` → 原样返回该路径 |
| `saveFile`（`data`） | 取消 → `{"canceled":true,"success":false}`；写入 `D:\ZCodium\.plan\e2e-save.txt` → `{"path":…,"success":true}`，文件 25 字节且内容逐字一致 |
| `saveFile`（`sourceUrl`） | **验收台没有该分支的入口**；由自动化测试覆盖不变量，未做人工核对 |
| `openInFileManager`（不存在） | `{"success":false,"error":"path_unavailable: 系统找不到指定的文件。 (os error 2)"}` |
| `openInFileManager`（目录/文件） | 均 `{"success":true}`；Explorer 窗口标题为 `D:\ZCodium`（文件打开父目录） |
| `showTaskNotification` | `(void)`；系统通知可见，标题/正文与入参一致（截图 `.plan/notification-shot.png`） |
| `notifyRendererReady` / `syncWindowTabs` / `syncWindowUnreadCount` | 均 `(void)` |
| `getApplicationIcon` | `null`（未抛错） |
| 深链 | 再启动一次 `ModeTauri.exe "mode://oauth/callback?state=test"` → 验收台显示 `oauth-callback  mode://oauth/callback?state=test` |

**门禁**：`cargo test -p platform -p protocol -p deeplink -p tray -p updater -p tauri-test-manifest`
全绿（6/2/1/4/…）；`clippy --all-targets -D warnings` 零告警。

**副作用与还原**：跑壳会把 `HKCU\Software\Classes\mode\shell\open\command` 临时改指到
`target/release/ModeTauri.exe`；已按 §4.3 的口径用 `.plan/mode-protocol-backup.reg` 还原，
复核当前值指回已安装的 `Mode.exe`。

**仍未验**：`selectDirectory` 的选择分支；`saveFile` 的 `sourceUrl` 分支（验收台未暴露）。
M1 阶段关闭仍只差 macOS/Linux 出包腿的首次 CI（需人推送）。
