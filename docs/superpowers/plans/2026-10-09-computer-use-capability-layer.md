# Computer Use 能力层（Windows Helper + Rust addon）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 交付电脑控制的「能力层」——broker 行协议、Rust 原生 addon（UIA/截图/输入/启动）、Helper 子进程入口与构建脚本，全部可独立测试；host 侧 runtime（14 工具分发、帧契约、resolver）由后续 Plan B 计划覆盖。

**Architecture:** 三层——(1) `@mode/cua/broker.js` 成为行协议唯一真源（客户端 `callBrokerMethod` 与服务端 `dispatchRequest` 同源）；(2) 新 crate `crates/mode-cua-ax`（napi-rs + windows-rs）在专用 UIA STA 线程上执行 8 个粗粒度原语；(3) 零依赖 ESM Helper 入口 `packages/mode-cua/helper/` 读 `--socket` 参数监听命名管道、按 fork IPC 约定发 `transport_ready/ready`、把 broker 方法 dispatch 到 addon。计划 A 不改任何 UI/插件/services 代码。

**Tech Stack:** 纯 ESM Node（零运行时依赖，`node --test`）、Rust 2021 + napi-rs 2 + windows-rs + image、esbuild 不需要（入口直拷）。

**Spec:** `docs/specs/computer-use-windows-runtime.md`（帧契约与 14 工具语义归 Plan B；本计划只消费其中「broker 协议」「构建发布」「风险」节）

## Global Constraints

- 纯净室：不读 `C:\Users\Administrator\AppData\Local\Programs\ZCode` 下任何官方实现；规范源=插件市场 docs/SDK、本仓消费方、Windows 公开 API。
- 错误码只有 17 键（SDK `ERROR_CODE_BY_BROKER` 逐字核对）：`permission_denied, not_authorized, launch_failed, invalid_request, element_unavailable, not_settable, not_selectable, action_unavailable, foreground_required, controller_busy, broker_unavailable, version_mismatch, stale_socket, timeout, unimplemented, method_not_found, internal`——多一个键都会被 SDK 归为 `INTERNAL`，少一个键会让语义降级。
- fork 线格式（已有 `parseReadyMessage` 消费）：`{protocol:"mode-cua-windows-dev/v1", type:"transport_ready"|"ready"|"error", socketPath, pid}`；argv 固定 `entry --socket <path> --parent-pid <n>`；env 固定 `MODE_CUA_HELPER_ADDON=<addonPath>`、`ELECTRON_RUN_AS_NODE=1`（见 `packages/services/src/cua-permission-broker/windowsCuaDevHelperHost.ts:313-328`，不得改动）。
- Helper socket 连接**没有口令**（`packages/shared/src/runtimeEnv.ts:149` 注释拍板）：socket 路径本身不可猜即可；`pluginAuthority` 只是 provenance。
- `packages/mode-cua` 保持零运行时依赖、纯 ESM `.mjs/.js`；`mode-cua` 不得反向依赖 `core/services`。
- win32 only；不接官方 Helper 下载链；日志/错误不落 token、不落用户屏幕内容。
- 每个任务收尾跑该层测试；涉及全仓口径时跑 `pnpm typecheck`、`pnpm lint`，如实报告结果。
- 提交只 `git add` 本任务文件；不 push。

---

### Task 1: broker 行协议与 socket 工具（`@mode/cua/broker.js`）

**Files:**
- Modify: `packages/mode-cua/broker.js`（替换 stub 导出为真实现，保持 `broker.d.ts` 声明的导出面）
- Modify: `packages/mode-cua/package.json`（加 `"test": "node --test test/*.test.mjs"`）
- Test: `packages/mode-cua/test/broker.test.mjs`（新建）

**Interfaces:**
- Consumes: 无（本任务是后续所有任务的协议底座）。
- Produces（Task 7/8 与 Plan B 依赖，签名以 `broker.d.ts` 为准）:
  - `mintBrokerSocketPath({dir?, env?}): string` — win32 返回 `\\.\pipe\mode-cua-<16hex>`，posix 返回 `<dir|tmpdir()>/mode-cua-<16hex>.sock`
  - `callBrokerMethod<T>({socketPath, method, params?, timeoutMs?}): Promise<T>` — 超时默认 30_000ms
  - `probeHelperHealth(socketPath, {timeoutMs=10_000, pollIntervalMs=100, perTryTimeoutMs=500}): Promise<{bundleId: string|null, pid: number|null}>`
  - `parseRequestLine/serializeResponse/okResponse/errorResponse/errorResponseFromException/dispatchRequest/handleRequestLine/isBrokerMethod/isReadOnlyBrokerMethod`
  - 错误类 `BrokerError`/`CuaHelperError`（已有，保留）

- [ ] **Step 1: 写失败测试**

```js
// packages/mode-cua/test/broker.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import net from "node:net";
import {
  mintBrokerSocketPath, callBrokerMethod, probeHelperHealth,
  parseRequestLine, okResponse, errorResponseFromException,
  serializeResponse, dispatchRequest, isBrokerMethod, isReadOnlyBrokerMethod,
} from "../broker.js";

test("mintBrokerSocketPath returns unpredictable win32 pipe path", () => {
  const a = mintBrokerSocketPath(); const b = mintBrokerSocketPath();
  assert.match(a, /^\\\\\.\\pipe\\mode-cua-[0-9a-f]{16}$/u);
  assert.notEqual(a, b);
});

test("line codec roundtrip and rejects garbage", () => {
  const req = parseRequestLine('{"id":"1","method":"health","params":{}}');
  assert.equal(req.method, "health");
  assert.equal(parseRequestLine("not json"), undefined);
  assert.equal(parseRequestLine('{"method":1}'), undefined);
  assert.deepEqual(JSON.parse(serializeResponse(okResponse({ pid: 7 }))),
    { ok: true, result: { pid: 7 } });
});

test("method whitelist: primitives only, read-only subset", () => {
  for (const m of ["health","list_apps","list_windows","observe","capture","perform","launch_app","screen_probe"]) {
    assert.ok(isBrokerMethod(m), m);
  }
  assert.ok(!isBrokerMethod("exec"));
  assert.ok(isReadOnlyBrokerMethod("observe") && !isReadOnlyBrokerMethod("perform"));
});

test("callBrokerMethod + probeHelperHealth against a live server", async () => {
  const socketPath = mintBrokerSocketPath();
  const server = net.createServer((sock) => {
    sock.on("data", (buf) => {
      const req = parseRequestLine(buf.toString("utf8"));
      const res = req.method === "health"
        ? okResponse({ bundleId: null, pid: process.pid })
        : okResponse({ pong: true });
      sock.write(serializeResponse(res) + "\n");
    });
  });
  await new Promise((r) => server.listen(socketPath, r));
  try {
    assert.deepEqual(await callBrokerMethod({ socketPath, method: "ping" }), { pong: true });
    assert.deepEqual(await probeHelperHealth(socketPath, { timeoutMs: 2_000 }),
      { bundleId: null, pid: process.pid });
  } finally { await new Promise((r) => server.close(r)); }
});

test("error responses throw BrokerError with code; dispatch wraps backend throws", async () => {
  const parsed = parseRequestLine('{"id":"1","method":"nope","params":{}}');
  assert.equal(parsed.method, "nope");
  const errResp = errorResponseFromException(Object.assign(new Error("boom"), { code: "timeout" }));
  assert.equal(errResp.ok, false);
  assert.equal(errResp.error.code, "timeout");
  const backend = { health: async () => { throw Object.assign(new Error("gone"), { code: "broker_unavailable" }); } };
  const res = await dispatchRequest(backend, { id: "1", method: "health", params: {} });
  assert.deepEqual(res, { ok: false, error: { code: "broker_unavailable", message: "gone" } });
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `pnpm --filter @mode/cua test`（先在 `packages/mode-cua/package.json` scripts 加 `"test": "node --test test/*.test.mjs"`）
Expected: FAIL（`mintBrokerSocketPath` 等导出为 stub 或不存在）

- [ ] **Step 3: 实现**

`broker.js` 保留既有错误类与工厂，替换/新增以下实现（要点，完整代码按此写）：

```js
import { randomBytes, timingSafeEqual } from "node:crypto";
import net from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

export function mintBrokerSocketPath({ dir, env } = {}) {
  const id = randomBytes(8).toString("hex");
  if (process.platform === "win32" && !dir) return `\\\\.\\pipe\\mode-cua-${id}`;
  return join(dir ?? tmpdir(), `mode-cua-${id}.sock`);
}
export function resolveBrokerSocketPath(o) { return mintBrokerSocketPath(o); } // 语义同源：路径即凭据

function serializeRequest(id, method, params) {
  return JSON.stringify({ id, method, params: params ?? {} });
}
export function parseRequestLine(line) {
  let v; try { v = JSON.parse(line); } catch { return undefined; }
  if (!v || typeof v !== "object" || typeof v.method !== "string") return undefined;
  return { id: typeof v.id === "string" ? v.id : null, method: v.method, params: v.params ?? {} };
}
export function okResponse(result) { return { ok: true, result }; }
export function errorResponse(message, { code = "internal" } = {}) {
  return { ok: false, error: { code, message } };
}
export function errorResponseFromException(e) {
  return errorResponse(e instanceof Error ? e.message : String(e),
    { code: (e && typeof e === "object" && typeof e.code === "string" && e.code) || "internal" });
}
export function serializeResponse(r) { return JSON.stringify(r); }

const BROKER_METHODS = new Set(["health","list_apps","list_windows","observe","capture","perform","launch_app","screen_probe"]);
const READONLY_METHODS = new Set(["health","list_apps","list_windows","observe","capture","screen_probe"]);
export const isBrokerMethod = (m) => BROKER_METHODS.has(m);
export const isReadOnlyBrokerMethod = (m) => READONLY_METHODS.has(m);

export async function dispatchRequest(backend, request) {
  if (!isBrokerMethod(request.method)) return errorResponse(`unknown method: ${request.method}`, { code: "method_not_found" });
  try { return okResponse(await backend[request.method](request.params ?? {})); }
  catch (e) { return errorResponseFromException(e); }
}
export async function handleRequestLine(backend, line) {
  const req = parseRequestLine(line);
  if (!req) return errorResponse("malformed request", { code: "invalid_request" });
  return await dispatchRequest(backend, req);
}

// 客户端：单请求单连接，行分隔 JSON；超时与对端错误一律转 BrokerError。
export function callBrokerMethod({ socketPath, method, params, timeoutMs = 30_000 }) {
  return new Promise((resolve, reject) => {
    const sock = net.connect(socketPath);
    let buf = ""; let done = false;
    const finish = (fn, v) => { if (done) return; done = true; clearTimeout(timer); sock.destroy(); fn(v); };
    const timer = setTimeout(() => finish(reject,
      new BrokerError("broker call timed out", { code: "timeout" })), timeoutMs);
    sock.on("connect", () => sock.write(serializeRequest("1", method, params) + "\n"));
    sock.on("data", (chunk) => {
      buf += chunk.toString("utf8");
      const nl = buf.indexOf("\n"); if (nl < 0) return;
      let resp; try { resp = JSON.parse(buf.slice(0, nl)); } catch (e) { return finish(reject, e); }
      if (resp && resp.ok === true) return finish(resolve, resp.result);
      const err = resp && resp.error ? resp.error : {};
      return finish(reject, new BrokerError(err.message ?? "broker error", { code: err.code ?? "internal" }));
    });
    sock.on("error", (e) => finish(reject, new BrokerError(e.message, { code: "stale_socket" })));
    sock.on("close", () => finish(reject, new BrokerError("connection closed", { code: "stale_socket" })));
  });
}

export async function probeHelperHealth(socketPath, options = {}) {
  const { timeoutMs = 10_000, pollIntervalMs = 100, perTryTimeoutMs = 500 } = options;
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const r = await callBrokerMethod({ socketPath, method: "health", timeoutMs: perTryTimeoutMs });
      if (r && (r.pid === null || typeof r.pid === "number")) {
        return { bundleId: typeof r.bundleId === "string" ? r.bundleId : null, pid: r.pid ?? null };
      }
      lastError = new Error("health payload invalid");
    } catch (e) { lastError = e; }
    await new Promise((r) => setTimeout(r, pollIntervalMs));
  }
  throw new BrokerError(`helper health probe failed: ${lastError?.message ?? "unknown"}`,
    { code: "broker_unavailable" });
}
```

注意：`callBrokerMethod` 的 `sock.on("close")` 兜底只在未完成时 reject；测试里的服务器每连接回一行即可通过。

- [ ] **Step 4: 跑测试确认通过**

Run: `pnpm --filter @mode/cua test`
Expected: PASS（5 个用例）

- [ ] **Step 5: 质量门 + 提交**

Run: `pnpm lint` && `pnpm typecheck`
Expected: PASS（如实记录；若 oxlint 对 `packages/mode-cua` 有既有告警，不扩大改动面）

```bash
git add packages/mode-cua/broker.js packages/mode-cua/package.json packages/mode-cua/test/broker.test.mjs
git commit -m "feat(cua): broker 行协议与 socket 工具真实现"
```

---

### Task 2: Rust crate 脚手架 + `version/list_apps/list_windows`

**Files:**
- Create: `crates/mode-cua-ax/Cargo.toml`
- Create: `crates/mode-cua-ax/src/lib.rs`（napi 导出 + 错误类型）
- Create: `crates/mode-cua-ax/src/apps.rs`（应用与窗口枚举）
- Create: `crates/mode-cua-ax/src/error.rs`（→ 17 码映射）
- Test: `crates/mode-cua-ax/tests/enumerate.rs`

**Interfaces:**
- Consumes: 无。
- Produces（Task 7/8 依赖的 addon 导出面，全部 `#[napi]`）:
  - `version() -> String`（semver 字符串，与 `@mode/cua` 包版本对齐演进）
  - `list_apps() -> Vec<AppInfoNapi>`：`{pid: u32, name: Option<String>, bundleId: Option<String>, active: bool}`（`bundleId` = AUMID，取不到为 null；`name` = exe 显示名）
  - `listWindows(pid: Option<u32>) -> Vec<WindowRowNapi>`：`{windowId: u32, pid: u32, title: Option<String>, bounds: [i32;4], main: bool, focused: bool, onscreen: bool}`
  - 错误统一 `AxError { code: &'static str(17码), message: String }`，napi 侧抛出携带 `code` 属性的 Error

- [ ] **Step 1: rustup 前置**

Run: `rustup show && rustup target add x86_64-pc-windows-msvc`
Expected: 已安装；缺则先装（报告环境受限时如实说明）

- [ ] **Step 2: scaffold**

```toml
# crates/mode-cua-ax/Cargo.toml
[package]
name = "mode-cua-ax"
version = "0.1.0"
edition = "2021"
license = "MIT"

[lib]
crate-type = ["cdylib"]

[dependencies]
napi = { version = "2", features = ["serde"] }
napi-derive = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
windows = { version = "0.58", features = [
  "Win32_Foundation", "Win32_UI_Accessibility", "Win32_UI_Input_KeyboardAndMouse",
  "Win32_UI_WindowsAndMessaging", "Win32_Graphics_Gdi", "Win32_System_Threading",
  "Win32_System_LibraryLoader", "Win32_UI_Shell", "Win32_System_Com",
  "Win32_System_DataExchange", "Win32_Security",
] }
image = { version = "0.25", default-features = false, features = ["png", "jpeg"] }

[build-dependencies]
napi-build = "2"

# crates/mode-cua-ax/build.rs
fn main() { napi_build::setup(); }
```

`src/error.rs`：

```rust
/// SDK `ERROR_CODE_BY_BROKER` 的全部 17 个键（与 docs/specs/computer-use-windows-runtime.md 同步维护）。
pub const ALLOWED_CODES: [&str; 17] = [
  "permission_denied", "not_authorized", "launch_failed", "invalid_request",
  "element_unavailable", "not_settable", "not_selectable", "action_unavailable",
  "foreground_required", "controller_busy", "broker_unavailable", "version_mismatch",
  "stale_socket", "timeout", "unimplemented", "method_not_found", "internal",
];
#[derive(Debug)]
pub struct AxError { pub code: &'static str, pub message: String }
impl AxError {
  pub fn new(code: &'static str, message: impl Into<String>) -> Self {
    debug_assert!(ALLOWED_CODES.contains(&code), "non-contract code: {code}");
    Self { code, message: message.into() }
  }
  pub fn internal(m: impl Into<String>) -> Self { Self::new("internal", m) }
}
impl std::fmt::Display for AxError { /* message */ }
impl std::error::Error for AxError {}
pub type AxResult<T> = Result<T, AxError>;
```

`src/lib.rs` 骨架（napi 错误转换）：

```rust
#![deny(clippy::unwrap_used)]
// 模块必须 pub：集成测试（tests/*.rs）直接调用纯逻辑函数，napi 包装只做错误转码。
pub mod apps; pub mod error;
pub mod observe; pub mod capture; pub mod perform; pub mod launch; pub mod screen;
pub mod uia_thread; pub mod input; pub mod clipboard;
use napi_derive::napi;
use error::{AxError, AxResult};

#[napi]
pub fn version() -> String { env!("CARGO_PKG_VERSION").to_string() }

#[napi(object)]
pub struct AppInfoNapi { pub pid: u32, pub name: Option<String>, pub bundle_id: Option<String>, pub active: bool }
#[napi(object)]
pub struct WindowRowNapi { pub window_id: u32, pub pid: u32, pub title: Option<String>,
  pub bounds: Vec<i32>, pub main: bool, pub focused: bool, pub onscreen: bool }

#[napi]
pub fn list_apps() -> napi::Result<Vec<AppInfoNapi>> { to_napi(apps::list_apps()) }

fn to_napi<T>(r: AxResult<T>) -> napi::Result<T> {
  r.map_err(|e| napi::Error::new(napi::Status::GenericError,
    format!("{}:{}", e.code, e.message)))
}
```

（napi 错误只带字符串，Task 8 的 JS 侧按 `code:message` 前缀拆回 17 码——拆分规则写进 Task 8。根级 `#[napi] list_apps` 与 `apps::list_apps` 是不同路径，无命名冲突；后续任务的 napi 包装同理。）

- [ ] **Step 3: 写失败测试**

```rust
// crates/mode-cua-ax/tests/enumerate.rs
use mode_cua_ax::*; // 经 napi 导出的 fn 不在 crate 根可直接调，改为把纯逻辑放 pub fn

#[test]
fn list_apps_returns_system_processes() {
  let apps = apps::list_apps().expect("enumerate");
  assert!(!apps.is_empty());
  assert!(apps.iter().any(|a| a.name.is_some()));
}
#[test]
fn list_windows_finds_own_test_window() { /* Task 3 fixture 就绪后在该任务补全；此处先断言桌面窗口存在 */
  let wins = apps::list_windows(None).expect("enumerate");
  assert!(!wins.is_empty());
}
```

说明：纯逻辑函数（`apps::list_apps`）用 `pub(crate)` 以外的 `pub` 暴露在模块内供集成测试；napi 包装只做错误转换。

- [ ] **Step 4: 实现 `apps.rs`**

要点（完整实现按此写）：
- 进程名：`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW` 取 exe 路径→文件名；活跃窗口判断用 `GetForegroundWindow` 的 pid。
- AUMID：对有窗口进程读 `GetApplicationUserModelId(processHandle)`（`Win32_UI_Shell`；失败为 `None`）。
- 窗口枚举：`EnumWindows` 过滤 `IsWindowVisible` + 非自身消息窗；`main` = 该进程第一个顶层窗口，`focused` = `GetForegroundWindow` 相等，`onscreen` 恒 true（Win32 无 CoreGraphics 语义），`title` 取不到时 `None`（**不是**空串——空串保留给「读到但为空」）。
- 错误全部走 `AxError`，绝不 `unwrap`（clippy `deny` 已开）。

- [ ] **Step 5: 跑测试**

Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml`
Expected: PASS（2 用例；枚举测试断言非空——Windows 上必成立）

- [ ] **Step 6: 提交**

```bash
git add crates/mode-cua-ax
git commit -m "feat(cua-ax): Rust addon 脚手架与应用/窗口枚举"
```

---

### Task 3: Rust `observe`（UIA 树，STA 线程）

**Files:**
- Create: `crates/mode-cua-ax/src/uia_thread.rs`（STA 命令线程）
- Create: `crates/mode-cua-ax/src/observe.rs`
- Create: `crates/mode-cua-ax/examples/probe.rs`（Week-0 探针：对指定窗口打印树）
- Test: `crates/mode-cua-ax/tests/observe.rs`（自建 fixture 窗口）

**Interfaces:**
- Consumes: Task 2 的错误类型与窗口枚举。
- Produces:
  - napi: `observe(req: ObserveRequestNapi) -> ObserveResultNapi`
  - `ObserveRequestNapi {windowId: u32, maxElements: u32}`（默认上限 3000）
  - `ObserveResultNapi {windowTitle: Option<String>, focusedIndex: Option<u32>, enumerationComplete: bool, elements: Vec<ElementNapi>}`
  - `ElementNapi {index: u32, kind: String, title: Option<String>, value: Option<String>, bounds: Vec<i32>, actions: Vec<String>, enabled: bool, offscreen: bool}`
  - `kind` = UIA ControlType 小写（`edit/button/text/window/list/listitem/menuitem/checkbox/combo/hyperlink/tab/treeitem/pane/document/...`）
  - `actions` = 支持的 pattern 动作名：`press`(Invoke) `toggle`(Toggle) `expand`/`collapse`(ExpandCollapse) `select`(SelectionItem) `scroll_into_view`(ScrollItem) + LegacyIAccessible DefaultAction 原文（去重、保序）

- [ ] **Step 1: Week-0 探针（风险前置）**

```rust
// crates/mode-cua-ax/examples/probe.rs — 手动运行：cargo run --example probe -- <hwnd十进制>
// 打印 observe 全树 JSON 到 stdout，用于先验证 UIA 对 Chromium/Electron 窗口的树质量。
```

Run: `cargo run --manifest-path crates/mode-cua-ax/Cargo.toml --example probe -- <设置页窗口hwnd>`
Expected: 树中出现可交互子元素（非仅窗口壳）；不达标 → 在本任务内记录结论并降级为「补 MSAA fallback」（见 spec 风险表），**先向用户报告探针结果再继续扩大实现**。

- [ ] **Step 2: 写失败测试（fixture 窗口，放共享模块供 Task 4/5 复用）**

```rust
// crates/mode-cua-ax/tests/common/mod.rs  —— 各测试文件 `mod common;` 引入
use windows::Win32::Foundation::{HWND, LRESULT, WPARAM, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;
use std::sync::mpsc;

/// 建一个顶层 STATIC 窗，内嵌 EDIT + BUTTON；独立线程跑消息泵；返回 hwnd。
/// Drop（`destroy()`）PostQuitMessage + join 线程，避免测试间窗口残留。
pub struct Fixture { pub hwnd: isize, tx: Option<mpsc::Sender<()>>, join: Option<std::thread::JoinHandle<()>> }
pub fn spawn_fixture() -> Fixture {
  let (ready_tx, ready_rx) = mpsc::channel();
  let join = std::thread::spawn(move || {
    unsafe {
      RegisterClassW(&WNDCLASSW { lpszClassName: "ModeCuaTest\0".into(),
        lpfnWndProc: Some(def_window_proc), hInstance: GetModuleHandleW(None).unwrap().into(), ..Default::default() });
      let hwnd = CreateWindowExW(WINDOW_EX_STYLE::default(), "ModeCuaTest\0", "fixture\0",
        WS_OVERLAPPEDWINDOW, 0, 0, 320, 240, None, None, None, None).unwrap();
      CreateWindowExW(WINDOW_EX_STYLE::default(), "EDIT\0", "hello\0", WS_CHILD | WS_VISIBLE,
        10, 10, 200, 24, hwnd, HMENU(1), None, None);
      CreateWindowExW(WINDOW_EX_STYLE::default(), "BUTTON\0", "press me\0", WS_CHILD | WS_VISIBLE,
        10, 50, 100, 28, hwnd, HMENU(2), None, None);
      ShowWindow(hwnd, SW_SHOW);
      let _ = ready_tx.send(hwnd.0 as isize);
      let mut msg = MSG::default();
      while GetMessageW(&mut msg, None, 0, 0).into() { let _ = TranslateMessage(&msg); DispatchMessageW(&msg); }
    }
  });
  let hwnd = ready_rx.recv().expect("fixture window ready");
  Fixture { hwnd, tx: None, join: Some(join) }
}
// destroy(): unsafe { DestroyWindow(HWND(self.hwnd)) } —— 线程收 WM_DESTROY 后 QuitMessage，join。

// crates/mode-cua-ax/tests/observe.rs
mod common;
#[test]
fn observe_indexes_fixture_controls() {
  let fix = common::spawn_fixture();
  let r = observe::observe(fix.hwnd as u32, 3000).expect("observe");
  assert!(r.elements.iter().any(|e| e.kind == "button"));
  assert!(r.elements.iter().any(|e| e.kind == "edit"));
  assert!(r.elements.iter().any(|e| e.actions.contains(&"press".to_string())));
  assert!(r.elements.iter().all(|e| e.bounds.len() == 4));
}
```

（Task 4/5 的测试同样 `mod common;` 复用 `spawn_fixture`；上方 `Fixture` 结构体在各测试尾部调用 `destroy()`。）

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml observe`
Expected: FAIL（observe 未实现）

- [ ] **Step 4: 实现**

`uia_thread.rs`：进程级单例 STA 线程（首次调用 spawn，`CoInitializeEx(COINIT_APARTMENTTHREADED)`），`mpsc::Sender<Cmd>` + `Cmd::Observe{...}` 等变体，调用方 `send` + `recv_timeout(30s)`；panic 或超时 → `AxError::new("timeout", ...)`。
`observe.rs`：
- `IUIAutomation::ElementFromHandle(hwnd)` → `FindAll(TreeScope_Subtree, TrueCondition)`，深度优先按 UIA 子序遍历（超过 `maxElements` 截断并置 `enumerationComplete=false`）。
- 每元素读 `CurrentControlType/CurrentName/CurrentBoundingRectangle/CurrentIsEnabled/CurrentIsOffscreen`；value 读 `ValuePattern.CurrentValue`（无 pattern 为 None）。
- `focusedIndex`：`GetFocusedElement` 的 RuntimeId 与遍历集合匹配（不匹配为 None）。
- actions：按 Produces 节的 pattern→动作名映射 + `IAccessible::get_accDefaultAction`（经 `CUIAutomation` 的 `LegacyIAccessiblePattern`）。
- ControlType→kind 用 `match` 全表映射，未知 → `"pane"`。

- [ ] **Step 5: 跑测试**

Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml`
Expected: PASS（含 Task 2 既有用例）

- [ ] **Step 6: 提交**

```bash
git add crates/mode-cua-ax
git commit -m "feat(cua-ax): UIA 观察原语与 STA 命令线程"
```

---

### Task 4: Rust `capture`（截图）

**Files:**
- Create: `crates/mode-cua-ax/src/capture.rs`
- Test: `crates/mode-cua-ax/tests/capture.rs`

**Interfaces:**
- Consumes: Task 3 的 STA 线程（GDI 调用同线程串行）。
- Produces:
  - napi: `capture(req: CaptureRequestNapi) -> CaptureResultNapi`
  - `CaptureRequestNapi {windowId: Option<u32>, region: Option<Vec<i32>> /*[x,y,w,h] 相对窗口*/, fullScreen: bool}`
  - `CaptureResultNapi {data: Buffer /*PNG*/, width: u32, height: u32, clamped: bool}`
  - 行为：`PrintWindow(hwnd, PW_RENDERFULLCONTENT)` → 失败回退 BitBlt 窗口矩形；`fullScreen` 用屏幕 DC；region 越界 clamp 并置 `clamped=true`；采样校验全黑/全空 → `AxError::new("internal", "screen capture returned a blank frame")`

- [ ] **Step 1: 写失败测试**

```rust
// crates/mode-cua-ax/tests/capture.rs
mod common;
#[test]
fn capture_fixture_window_png_nonzero() {
  let fix = common::spawn_fixture();
  let r = capture::capture(Some(fix.hwnd as u32), None, false).expect("capture");
  assert!(r.data.len() > 1024);
  assert_eq!(&r.data[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
  assert!(r.width > 0 && r.height > 0);
}
#[test]
fn region_outside_window_is_clamped() {
  let fix = common::spawn_fixture();
  let r = capture::capture(Some(fix.hwnd as u32), Some(vec![ -50, -50, 10_000, 10_000 ]), false).expect("capture");
  assert!(r.clamped);
}
```

- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml capture` → Expected: FAIL

- [ ] **Step 3: 实现** — `PrintWindow` 到 32bpp DIB（`PW_RENDERFULLCONTENT=3`），失败 BitBlt；region clamp 到位图尺寸；`image::load_from_memory` 不用——直接 `image::codecs::png::PngEncoder` 写 RGB；全黑检测：解码后逐像素 stride 采样（步长=宽/64），全 0 → `internal` 错误。

- [ ] **Step 4: 跑测试** — Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml` → Expected: PASS

- [ ] **Step 5: 提交** — `git add crates/mode-cua-ax && git commit -m "feat(cua-ax): 窗口/全屏截图与 region clamp"`

---

### Task 5: Rust `perform`（输入注入）

**Files:**
- Create: `crates/mode-cua-ax/src/input.rs`（keysym 表、SendInput 封装、前台校验）
- Create: `crates/mode-cua-ax/src/clipboard.rs`
- Create: `crates/mode-cua-ax/src/perform.rs`（动作分发）
- Test: `crates/mode-cua-ax/tests/perform.rs`

**Interfaces:**
- Consumes: Task 3 的 observe（目标解析与前台校验）、错误类型。
- Produces:
  - napi: `perform(req: PerformRequestNapi) -> PerformResultNapi`
  - `PerformRequestNapi {kind: String, windowId: u32, payload: serde_json::Value}`，`kind ∈ click|click_drag|scroll|key|type_text|set_value|select_text|action|paste`
  - payload 形状：`click {x,y,button,clickCount,modifiers}` / `click_drag {fromX,fromY,toX,toY,modifiers}` / `scroll {x,y,direction,amount}` / `key {chord,repeat,holdMs}` / `type_text {text}` / `set_value {elementIndex|"x,y"焦点, value}` / `select_text {elementIndex, start, length}` / `action {elementIndex, action}` / `paste {text}`（`elementIndex`/坐标由 Task 8 的 helper 侧解析后传原生坐标——**决策：坐标解析放 helper JS，addon 只收绝对坐标与原始键序列**，见 Step 4）
  - `PerformResultNapi {dispatched: String /* "dispatched" | "not_dispatched" | "unknown" */}`
  - `event` 策略前台校验：`GetForegroundWindow() != hwnd` → `AxError::new("foreground_required", ...)`

- [ ] **Step 1: 写失败测试**

```rust
// crates/mode-cua-ax/tests/perform.rs
mod common;
#[test]
fn keysym_chord_parses() {
  assert_eq!(input::resolve_chord("Return").unwrap(), vec![input::KeyStep::Tap(0x0D)]);
  // 组合展开：ctrl↓ shift↓ t↓ t↑ shift↑ ctrl↑ = 6 步（修饰键包住主键，抬起顺序镜像）
  assert_eq!(input::resolve_chord("ctrl+shift+t").unwrap().len(), 6);
  assert!(input::resolve_chord("NotAKey").is_err());
}
#[test]
fn scroll_amount_clamped_to_100_pages() {
  // docs 语义是 clamp 不是拒绝：>100 按 100、负数按 0
  assert_eq!(input::wheel_delta_for_pages(101).unwrap(), 12_000); // 120 * 100
  assert_eq!(input::wheel_delta_for_pages(2).unwrap(), 240);      // WHEEL_DELTA(120) * pages
  assert_eq!(input::wheel_delta_for_pages(0).unwrap(), 0);
}
#[test]
fn click_on_fixture_reports_dispatched() {
  let fix = common::spawn_fixture();
  let r = perform::perform(&perform::Req::Click { x: 10, y: 10, button: "left", click_count: 1, modifiers: "" }, fix.hwnd as u32).unwrap();
  assert_eq!(r, "dispatched");
}
```

- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml perform` → Expected: FAIL

- [ ] **Step 3: 实现**
- `resolve_chord`：split `+`；token 表 `Return/Tab/Escape/BackSpace/Delete/Up/Down/Left/Right/Home/End/Page_Up/Page_Down/Space/F1..F12/Control_L/Control_R/Shift_L/Shift_L/Alt_L/super/ctrl/alt`（Windows 上 `super`→VK_LWIN、`ctrl`→VK_CONTROL）；单字符 → `VkKeyScanW`；产出 `Vec<KeyStep::{Down(u16), Up(u16), Unicode(char)}>`；未知 → `AxError::new("invalid_request", ...)`。
- 点击/拖拽：`SendInput` 鼠标事件，绝对坐标经 `MOUSEEVENTF_ABSOLUTE|VIRTUAL_DESK` 归一化（`x*65535/(w-1)`，多显示器用虚拟屏矩形）；`clickCount=2` 中间 50ms；modifiers 在按下期间真实按下/抬起。
- 滚动：`MOUSEEVENTF_WHEEL`（右向 `MOUSEEVENTF_HWHEEL`），delta=120*amount。
- `type_text`：逐字符 `KEYEVENTF_UNICODE`（IME 组合不做——spec 定为一期范围外）。
- `set_value` 的 event 兜底与 `select_text` 的 UIA TextPattern 属 observe 配合路径：本任务先实现「焦点元素全选+键入」辅助 `select_all_and_type`（由 Task 8 编排）；TextPattern Select 在 observe 的 pattern 能力探测里已知可用时才走 UIA 路径——UIA ValuePattern/TextPattern 的原生调用一并放本文件（`invoke_value_pattern(element_ptr,...)`）。
- `paste`：`clipboard.rs` 保存 CF_UNICODETEXT → 写入新文本 → `Ctrl+V` → sleep(150ms) → 还原剪贴板；目标焦点丢失 → `foreground_required`。
- dispatched 判定：`SendInput` 返回写入条数==期望 → `dispatched`；返回 0 → `not_dispatched`；部分成功/超时 → `unknown`。UIA pattern 调用成功 → `dispatched`，`E_ELEMENTNOTAVAILABLE` → `element_unavailable`（AxError 直接抛）。

- [ ] **Step 4: 跑测试** — Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml` → Expected: PASS

- [ ] **Step 5: 提交** — `git add crates/mode-cua-ax && git commit -m "feat(cua-ax): SendInput 输入注入、keysym 与剪贴板粘贴"`

---

### Task 6: Rust `launch_app` + `screen_probe`

**Files:**
- Create: `crates/mode-cua-ax/src/launch.rs`
- Create: `crates/mode-cua-ax/src/screen.rs`
- Test: `crates/mode-cua-ax/tests/launch_screen.rs`

**Interfaces:**
- Consumes: Task 2 的进程枚举。
- Produces:
  - napi: `launchApp(req: LaunchRequestNapi) -> AppInfoNapi`
  - `LaunchRequestNapi {name: Option<String>, bundleId: Option<String>}`（二选一，SDK 保证；裸字符串按 bundleId 传入）
  - 解析顺序：`bundleId` 含 `.` → `IApplicationActivationManager::ActivateApplication(AUMID)`；否则 `SearchPathW(name.exe)`；再否则扫 `%APPDATA%\Microsoft\Windows\Start Menu` + `ProgramData\...\Start Menu` 的 `.lnk`（`IShellLinkW`+`IPersistFile`）匹配显示名 → `ShellExecuteW`。全失败 → `AxError::new("launch_failed", ...)`；参数非法 → `invalid_request`
  - napi: `screenProbe() -> ScreenProbeNapi {locked: bool}` — `OpenInputDesktop(0, FALSE, GENERIC_READ)` 失败即锁屏

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn screen_probe_reports_unlocked_in_ci() { assert!(!screen::probe().unwrap().locked); }
#[test]
fn launch_search_path_finds_notepad() {
  let app = launch::launch(&launch::Target::Name("notepad".into())).unwrap();
  assert!(app.name.unwrap().to_lowercase().contains("notepad"));
}
#[test]
fn launch_unknown_name_is_launch_failed() {
  assert_eq!(launch::launch(&launch::Target::Name("definitely-not-an-app-xyz".into())).unwrap_err().code, "launch_failed");
}
```

（`launch_search_path_finds_notepad` 在无 notepad 的环境会失败——Windows Server 桌面栈自带 notepad.exe；若 CI 缺失则该用例 `#[ignore]` 并在实机清单验收，如实标注。）

- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml launch` → Expected: FAIL

- [ ] **Step 3: 实现**（按 Produces 顺序；`spawn` 后 `EnumWindows` 轮询 ≤3s 等新窗口出现以回填 pid，超时仍返回进程启动成功的 AppInfo）

- [ ] **Step 4: 跑测试** — Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml` → Expected: PASS

- [ ] **Step 5: 提交** — `git add crates/mode-cua-ax && git commit -m "feat(cua-ax): 应用启动与锁屏探测"`

---

### Task 7: Helper 入口——IPC 握手、pipe 服务、addon 装载

**Files:**
- Create: `packages/mode-cua/helper/entry.mjs`（argv/生命周期/IPC）
- Create: `packages/mode-cua/helper/server.mjs`（pipe 服务端 + health）
- Create: `packages/mode-cua/helper/addon.mjs`（`MODE_CUA_HELPER_ADDON` 装载与错误转码）
- Test: `packages/mode-cua/test/helper-entry.test.mjs`

**Interfaces:**
- Consumes: Task 1（`parseRequestLine/handleRequestLine/okResponse/errorResponseFromException/serializeResponse`）、Task 2–6 的 addon 导出。
- Produces:
  - fork IPC：`process.send({protocol:"mode-cua-windows-dev/v1", type:"transport_ready", socketPath, pid})` → 监听就绪后 `{type:"ready", ...}`；启动失败 `{type:"error", error}`（字段名与 `packages/services/src/cua-permission-broker/windowsCuaHelperHost.ts:186-214` 的 `parseReadyMessage` 逐字对齐——实现前先读该函数，以它消费的字段为准）。
  - pipe 服务：单行 JSON 请求 → `handleRequestLine(backend)`；`backend` = 8 方法到 addon 的映射。
  - addon 装载：`createRequire(import.meta.url)(process.env.MODE_CUA_HELPER_ADDON)`；缺 env/装载失败 → 发 `type:"error"` 并以码 `internal` 退出（不得带绝对路径之外的敏感信息进 message？——路径属诊断必需，允许；不含 token）。
  - `--parent-pid`：轮询 `process.kill(ppid, 0)`（每 2s），父亡 → 直接 `process.exit(0)`（孤儿回收兜底，配合已有 `reapOrphanedHelpers`）。

- [ ] **Step 1: 写失败测试**

```js
// packages/mode-cua/test/helper-entry.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { fork } from "node:child_process";
import { writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { callBrokerMethod, mintBrokerSocketPath } from "../broker.js";

const entry = fileURLToPath(new URL("../helper/entry.mjs", import.meta.url));

// fake addon：满足 Task 8 的 backend 契约，不依赖真 UIA
const fakeAddonPath = join(mkdtempSync(join(tmpdir(), "cua-fake-")), "fake-addon.cjs");
writeFileSync(fakeAddonPath, `
  module.exports = {
    version: () => "0.0.0-test",
    health: () => ({ bundleId: null, pid: process.pid }),
    list_apps: () => [{ pid: 1, name: "fake.exe", bundle_id: null, active: false }],
    list_windows: () => [], observe: () => {}, capture: () => {},
    perform: () => ({ dispatched: "dispatched" }), launch_app: () => {}, screen_probe: () => ({ locked: false }),
  };
`);

test("helper handshakes and serves health over the pipe", { skip: process.platform !== "win32" }, async () => {
  const socketPath = mintBrokerSocketPath();
  const child = fork(entry, ["--socket", socketPath, "--parent-pid", String(process.pid)],
    { stdio: ["ignore", "pipe", "pipe", "ipc"], env: { ...process.env, MODE_CUA_HELPER_ADDON: fakeAddonPath } });
  const ready = await new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error("no ready in 5s")), 5_000);
    child.on("message", (m) => { if (m?.type === "ready") { clearTimeout(t); resolve(m); } });
    child.on("message", (m) => { if (m?.type === "error") { clearTimeout(t); reject(new Error(JSON.stringify(m.error))); } });
  });
  assert.equal(ready.protocol, "mode-cua-windows-dev/v1");
  assert.equal(ready.socketPath, socketPath);
  const health = await callBrokerMethod({ socketPath, method: "health", timeoutMs: 2_000 });
  assert.equal(typeof health.pid, "number");
  const apps = await callBrokerMethod({ socketPath, method: "list_apps", timeoutMs: 2_000 });
  assert.equal(apps[0].name, "fake.exe");
  child.kill();
});
```

- [ ] **Step 2: 跑测试确认失败** — Run: `pnpm --filter @mode/cua test` → Expected: FAIL（无入口）

- [ ] **Step 3: 实现**
- `entry.mjs`：解析 `--socket/--parent-pid` → `loadAddon(env)` → `createServer` on socketPath → 发 `transport_ready` → 首次 `listen` 成功发 `ready`（`pid: process.pid`）→ 挂 parent 轮询与 `SIGTERM/SIGINT` 优雅关（`server.close()` + `process.exit(0)`）。
- `server.mjs`：`sock.on("data")` 按行缓冲（单请求单响应，复用 Task 1 的解析），`handleRequestLine(backend, line)` → 写回一行；backend 工厂 `createBackend(addon)` 把 8 方法映射到 addon 函数（Task 8 完成全部映射，本任务先接 `health/list_apps`）。
- `addon.mjs`：装载失败抛 `AxError 风格 {code:"internal"}`。

- [ ] **Step 4: 跑测试** — Run: `pnpm --filter @mode/cua test` → Expected: PASS

- [ ] **Step 5: 提交** — `git add packages/mode-cua/helper packages/mode-cua/test && git commit -m "feat(cua): helper 入口 IPC 握手与命名管道服务"`

---

### Task 8: Helper backend——8 原语映射与 17 码转码

**Files:**
- Modify: `packages/mode-cua/helper/server.mjs`（补全 `createBackend`）
- Create: `packages/mode-cua/helper/errors.mjs`（napi 错误字符串 → `{code,message}`）
- Test: `packages/mode-cua/test/helper-backend.test.mjs`

**Interfaces:**
- Consumes: Task 2–6 全部 addon 导出；Task 7 的 backend 骨架。
- Produces（Plan B 的 runtime 直接依赖的 broker 方法语义）:
  - `health()` → `{bundleId:null, pid}`（HelperHealth 形状）
  - `list_apps(params)` / `list_windows({pid?})` → addon 原样数组
  - `observe({windowId, maxElements?})`、`capture({windowId?, region?, fullScreen?})`、`launchApp`→`launch_app({name?,bundleId?})`、`screen_probe()` → addon 结果
  - `perform({kind, windowId, payload})` → `{dispatched}`
  - **错误转码**：napi Error message 形如 `"<code>:<human message>"`，`errors.mjs` 的 `parseAxError(err)` 拆出 17 码之一（前缀不匹配 → `internal`），`dispatchRequest` 的 `errorResponseFromException` 需要拿到 `code` ——实现方式：backend 内 catch 后 `Object.assign(new Error(msg), {code})` 再抛（`errorResponseFromException` 已读 `err.code`，见 Task 1）。

- [ ] **Step 1: 写失败测试**（fake addon 注入，不依赖真 UIA）

```js
// packages/mode-cua/test/helper-backend.test.mjs
import test from "node:test"; import assert from "node:assert/strict";
import { createBackend } from "../helper/server.mjs";
import { parseAxError } from "../helper/errors.mjs";
import { dispatchRequest } from "../broker.js";

test("parseAxError splits the 17-code prefix", () => {
  const e = new Error("element_unavailable: row vanished");
  assert.deepEqual(parseAxError(e), { code: "element_unavailable", message: "row vanished" });
  assert.equal(parseAxError(new Error("no prefix here")).code, "internal");
});
test("backend maps perform and rewraps addon errors with code", async () => {
  const addon = {
    perform: async () => { throw new Error("foreground_required: app is background"); },
    health: () => ({ bundleId: null, pid: 42 }),
  };
  const res = await dispatchRequest(createBackend(addon), { id: "1", method: "perform", params: {} });
  assert.deepEqual(res, { ok: false, error: { code: "foreground_required", message: "app is background" } });
  const ok = await dispatchRequest(createBackend(addon), { id: "2", method: "health", params: {} });
  assert.deepEqual(ok, { ok: true, result: { bundleId: null, pid: 42 } });
});
```

- [ ] **Step 2: 跑测试确认失败** — Run: `pnpm --filter @mode/cua test` → Expected: FAIL

- [ ] **Step 3: 实现** — `createBackend(addon)` 逐方法包装（参数缺省、`windowId` 必填校验→`invalid_request`）；`errors.mjs` 的码表 = spec 17 键常量数组，用 `Set` 校验。

- [ ] **Step 4: 跑测试** — Run: `pnpm --filter @mode/cua test` → Expected: PASS（Task 1/7/8 全绿）

- [ ] **Step 5: 提交** — `git add packages/mode-cua/helper packages/mode-cua/test && git commit -m "feat(cua): helper backend 原语映射与错误码转译"`

---

### Task 9: 构建脚本 + manifest + dev 根契约

**Files:**
- Create: `scripts/build-cua-helper.mjs`
- Modify: `packages/mode-cua/package.json`（`modeCuaRuntime` 契约 + `files` + gitignore）
- Modify: `.gitignore`（`packages/mode-cua/dist-cua-helper/`、`crates/mode-cua-ax/target/`）
- Test: `packages/services/test/windowsCuaDevRootContract.test.ts`（借用既有 `resolveWindowsCuaRuntime` 做冒烟）

**Interfaces:**
- Consumes: Task 2–7 产物。
- Produces:
  - `pnpm build:cua-helper`（根 package.json scripts 增一行）→ 产出
    `packages/mode-cua/dist-cua-helper/{entry.cjs, cua_ax.node, runtime-manifest.json}`
  - manifest 字段逐字满足 `resolveWindowsCuaRuntime` 的 `validateRuntimeManifest`：
    `schemaVersion:1, packageName:"@mode/cua", packageVersion, platform:"win32", arch:"x64",
     electronVersion:<env MODE_CUA_ELECTRON_VERSION 或桌面 package.json 的 electron 版本>,
     entry:"entry.cjs", addon:"cua_ax.node", sha256{entry, addon}`
  - `packages/mode-cua/package.json` 增 `"modeCuaRuntime": {"schema":1,"windows":{"entry":"dist-cua-helper/entry.cjs","nativeAddon":"dist-cua-helper/cua_ax.node"}}`
  - entry.cjs 由 `helper/entry.mjs` 转写：脚本用 esbuild（repo 已有 tsup/esbuild 依赖）bundle 成单文件，`platform:"node", format:"cjs"`；无第三方依赖时基本是包一层。

- [ ] **Step 1: 写失败测试**

```ts
// packages/services/test/windowsCuaDevRootContract.test.ts
import test from "node:test"; import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { resolveWindowsCuaRuntime } from "../src/cua-permission-broker/windowsCuaDevRuntime.js";
// 前置：先跑 pnpm build:cua-helper 生成 dist-cua-helper

test("MODE_CUA_DEV_ROOT resolves the built helper", async () => {
  const r = await resolveWindowsCuaRuntime({
    platform: "win32", arch: "x64",
    env: { ...process.env, MODE_CUA_DEV_ROOT: fileURLToPath(new URL("../../packages/mode-cua", import.meta.url)) },
    fileSystem: undefined, // 用真实 fs 默认
  } as never);
  assert.match(r.entryPath, /entry\.cjs$/u);
  assert.match(r.addonPath, /cua_ax\.node$/u);
  assert.equal(r.commandEnv.ELECTRON_RUN_AS_NODE, "1");
});
```

（`resolveWindowsCuaRuntime` 的参数形状以 `windowsCuaDevRuntime.ts:78` 实签名为准，测试按真实签名微调；这是「借真实消费方锁契约」的测试，红了先改测试适配签名，再确认实现侧确实能解析我们的产物。）

- [ ] **Step 2: 跑测试确认失败** — Run: `pnpm --filter @mode/services test`（脚本 `tsx --test test/*.test.ts`）→ Expected: FAIL（dist 不存在/契约缺字段）

- [ ] **Step 3: 实现 `scripts/build-cua-helper.mjs`**

流程（同步骤真实代码要点）：
1. `runCommand("cargo", ["build","--release","--target","x86_64-pc-windows-msvc","--manifest-path","crates/mode-cua-ax/Cargo.toml"])`
2. 复制 `target/x86_64-pc-windows-msvc/release/mode_cua_ax.dll` → `packages/mode-cua/dist-cua-helper/cua_ax.node`
3. esbuild bundle `helper/entry.mjs` → `dist-cua-helper/entry.cjs`（`platform:node, format:cjs, target:node22, bundle:true`）
4. 读桌面 electron 版本（`require("./packages/desktop/package.json").devDependencies?.electron ?? dependencies.electron`，env `MODE_CUA_ELECTRON_VERSION` 覆盖）
5. 对 3、2 两文件算 `sha256` → 写 `runtime-manifest.json`
6. 打印三文件清单与 sha（不打绝对路径之外的机器信息）

- [ ] **Step 4: 跑构建 + 测试**

Run: `pnpm build:cua-helper && pnpm --filter @mode/services test`
Expected: 三产物生成；契约测试 PASS

- [ ] **Step 5: 提交**

```bash
git add scripts/build-cua-helper.mjs package.json packages/mode-cua/package.json .gitignore packages/services/test/windowsCuaDevRootContract.test.ts
git commit -m "feat(cua): helper 构建脚本、runtime-manifest 与 dev 根契约"
```

---

### Task 10: 端到端集成测试（真 helper vs 记事本）+ 全仓验证

**Files:**
- Create: `packages/mode-cua/test/integration.helper.test.mjs`（`CUA_INTEGRATION=1` 门）
- Modify: `packages/mode-cua/package.json`（`"test:integration": "node --test test/integration.helper.test.mjs"`；环境变量由调用方注入，避免 Windows cmd 与 POSIX 语法分叉）

**Interfaces:**
- Consumes: Task 1–9 全部。
- Produces: 能力层验收证据；Plan B 将在本链路上接 runtime。

- [ ] **Step 1: 写集成测试**

```js
// packages/mode-cua/test/integration.helper.test.mjs
const skip = process.env.CUA_INTEGRATION !== "1" || process.platform !== "win32";
test("spawn → observe → click → type roundtrip on notepad", { skip }, async () => {
  // 1) spawn entry（同 Task 7 参数），等 ready
  // 2) probeHelperHealth 通过
  // 3) launch_app {name:"notepad"} → pid
  // 4) list_windows {pid} → windowId（title 含 "Untitled" 或系统等价）
  // 5) observe → 断言含 kind:"edit"
  // 6) perform click(edit 绝对坐标) + type_text "cua-integration" → dispatched
  // 7) observe → edit 的 value 含 "cua-integration"
  // 8) 杀子进程，断言 socket 关闭、callBrokerMethod 抛 stale_socket
});
```

- [ ] **Step 2: 跑集成** — Run: `CUA_INTEGRATION=1 pnpm --filter @mode/cua run test:integration`（bash 注入；Windows cmd 下用 `set CUA_INTEGRATION=1 &&`）→ Expected: PASS（记事本窗口/输入法环境受限时如实记录失败证据，不注水）

- [ ] **Step 3: 全仓验证**

Run: `pnpm typecheck` && `pnpm lint` && `pnpm architecture:check --changed` && `pnpm --filter @mode/cua test`
Expected: 如实报告（架构检查若对 `packages/mode-cua/helper` 新增依赖边报错，按提示读 `pnpm architecture:context` 修正边界）

- [ ] **Step 4: 提交**

```bash
git add packages/mode-cua
git commit -m "test(cua): helper 端到端集成测试（记事本往返）"
```

---

## 后续计划（不在本文件范围）

- **Plan B — host runtime**：`frame-contract` 六函数、`createComputerUseRuntime`（14 工具、会话/state_id/diff、收据与 CUA_NOT_READY）、`createCuaProductMcpServerResolver`、display 黄金样例。spec 对应节：「runtime 面」「帧契约」「工具面」「controller lease」。
- **Plan C — 发布与验收**：electron-builder `tools/cua-helper` 打包、CI rust 接入、desktop E2E、实机验收清单、NOTICE 文案更新（占位声明改为可用声明需同步 NOTICE 两语言版）。
