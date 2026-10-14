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

- [ ] **Step 1: 写 workspace 根与两个 crate 骨架**

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

- [ ] **Step 2: 跑 workspace 解析**

Run: `cargo metadata --no-deps --format-version 1 > /dev/null && echo OK`
Expected: `OK`（注意：`crates/mode-cua-ax` 与 `crates/browser-import-helper` 会一并成为成员，
若它们的 `[package]` 与 workspace 冲突，改在其 Cargo.toml 显式声明而非改根）

- [ ] **Step 3: 骨架 main + 空窗口**

```rust
// apps/desktop-tauri/src-tauri/src/main.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 4: 本机构建**

Run: `cargo build --manifest-path apps/desktop-tauri/src-tauri/Cargo.toml`
Expected: 成功（首次拉取 crates.io 依赖，耗时较长）

- [ ] **Step 5: CI job（`tauri-skeleton`）与提交**

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

- [ ] **Step 1: 写失败的测试**（对纯粹可测的部分：命令名注册表与参数往返）

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

- [ ] **Step 2: 跑测试确认失败** Run: `cargo test -p platform` Expected: FAIL（未定义）
- [ ] **Step 3: 逐个实现**：每个命令先写 `tauri::test` 级测试再实现（红→绿）；
  `selectFile`/`selectDirectory`/`saveFile` 走 `tauri-plugin-dialog`，`openExternal`/`openInFileManager`
  走 `tauri-plugin-opener`，`showTaskNotification` 走 `tauri-plugin-notification`。
- [ ] **Step 4: 跑测试确认通过** Run: `cargo test -p platform && cargo clippy -p platform -- -D warnings`
- [ ] **Step 5: 提交** `git commit -m "feat(tauri): M1 平台命令面 10 命令（TDD）"`

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

- [ ] **Step 1: 写往返测试**（serde 序列化与 TS 侧同名 JSON 互认）
- [ ] **Step 2: 实现结构体 + `#[derive(JsonSchema, Serialize, Deserialize)]`**
- [ ] **Step 3: 生成 TS 并断言无漂移**

Run: `node scripts/generate-tauri-protocol.mjs --check`
Expected: 首次生成后写入文件；再次运行无 diff

- [ ] **Step 4: 提交** `git commit -m "feat(tauri): 协议 schemars/typeshare 双端生成与漂移断言"`

---

### Task 4: UI 挂载（DR-02）

**Files:**
- Modify: `apps/desktop-tauri/src-tauri/tauri.conf.json`（`build.frontendDist` 指向 renderer 产物）
- Create: `packages/ui/src/platform/tauri-invoke-shim.ts`（`window.mode` → `invoke()` 适配层）

**Interfaces:**
- Consumes: 存量 renderer 产物；`IPlatformService` 的 10 个方法。
- Produces: `createTauriPlatformService(): Partial<IPlatformService>`（**只实现 10 个**，
  其余方法显式抛 `not_implemented_until_m3`，不得静默返回假值）。

- [ ] **Step 1: 写失败的测试**：shim 对未实现方法必须抛错（不得 resolve undefined）。
- [ ] **Step 2: 实现 shim 并挂载 frontendDist。**
- [ ] **Step 3: 本机跑 E2E 场景**：开窗 → UI 渲染 → 依次触发 10 个命令，人工核对。
- [ ] **Step 4: 提交** `git commit -m "feat(tauri): 存量 UI 挂载与 invoke 适配层"`

---

### Task 5: 三平台出包与体积基线

**Files:**
- Modify: `.github/workflows/release.yml`（`tauri build` job，产物名 `Mode-<版本>-<平台>-<架构>.<ext>`）
- Create: `docs/specs/tauri-platform-commands.md` 附录（三平台体积记录）

- [ ] **Step 1: 加 `tauri build` job**（三平台矩阵，与旧线产物名一致，避免 Release 说明两套）。
- [ ] **Step 2: 记录体积**：Windows NSIS ≤30MB（争取 25MB）；mac/linux 同表记录。
- [ ] **Step 3: 提交** `git commit -m "ci(tauri): 三平台出包与体积基线"`

---

### Task 6: 更新链 stub

**Files:**
- Create: `crates/updater/src/lib.rs`、`crates/deeplink/src/lib.rs`、`crates/tray/src/lib.rs`

- [ ] **Step 1: `tauri-plugin-updater` 接现有 GitHub Releases 清单格式**（与旧线共用 feed，版本号延续）。
- [ ] **Step 2: deep-link 与 tray 落最小可用面**（`onOAuthCallback` 用 deep-link 打通）。
- [ ] **Step 3: 提交** `git commit -m "feat(tauri): updater/deeplink/tray 最小面"`

---

## 阶段验收

1. 三平台安装包全绿且 Windows ≤30MB（争取 25MB）；
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
