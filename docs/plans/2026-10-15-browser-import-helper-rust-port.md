# browser-import-helper C#→Rust 重写 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或 `executing-plans`
> 逐任务实现本计划。步骤用 `- [ ]` 复选框跟踪。

**Goal:** 把 `packages/desktop/native/windows-browser-import-helper/Program.cs`（1561 行 C#）替换为
`crates/browser-import-helper`（Rust），使 GitHub 语言条 C# 归零，并让路线图 M0 得以关闭。

**Architecture:** 三阶段。P1 只做**可本机验证**的层：oracle 对拍 harness + CLI 面 + broker wire 协议
编解码，全部用 fixture 驱动、无副作用。P2 做需要真实 Windows 会话的能力：提权、服务通道、密钥解封，
行为只受契约 B 的对外 reason 集合约束。P3 做接线与收尾：构建脚本、打包、删 C#、CI。

**Tech Stack:** Rust（edition 2021）、`windows` crate（与 `crates/mode-cua-ax` 同版本族 0.58）、
serde_json、clap 或手写解析（对齐既有精度要求，见 Task 3）、Node 24 + pnpm 10（harness 与构建脚本）。

**Spec:** `docs/specs/browser-import-helper-contract.md`（唯一取材来源）。
纪律：`docs/specs/clean-room-policy.md`——**禁止打开 Program.cs**；只允许读契约 spec、公开 API 文档、
以及运行旧产物观察到的输入输出。

## Global Constraints

- 语言政策：新增代码只允许 Rust / TS / HTML / CSS；`.cs` 不得新增。落地后 C# 文件从
  `scripts/language-policy-allowlist.txt` 移除对应行（只减不增）。
- 协议名 `ZCODE_BROWSER_IMPORT_HELPER` 与协议版本 `2` 是冻结常量，逐字节保持（契约 A）。
- stderr 不是契约（宿主 `stdio[2] = "ignore"`）：任何诊断信息不得只写 stderr 就当已交付。
- 本机（`win32 10.0.19044`）**无 dotnet/csc**，但 `C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe`
  存在，`pnpm --filter @mode/desktop prepare:browser-import-helper` 可产出 oracle（已验证）。
- 开发机（Git Bash，2026-10-15 实测）**无可用 Rust 链接器**：MSVC 未安装，且 `/usr/bin/link`
  （coreutils）会抢占 `link.exe` 名字；gnu 目标又缺 `x86_64-w64-mingw32-gcc`。
  故本机只能 `cargo check` / `cargo clippy`（均绿）；**单元测试必须在 CI Windows 腿执行**
  （已在 `release.yml` 加 `browser-import helper crate tests and clippy` 步骤）。
  要在本机跑测试需先装 VS Build Tools 或 mingw-w64——**未执行，属需人拍板的环境变更**。
- 不得为「让测试过」放宽契约；失败一律 fail-closed 并映射契约 B 的 reason。

---

## P1：可本机验证的层

### Task 1: oracle 对拍 harness

**Files:**
- Create: `scripts/browser-import-helper-oracle.mjs`
- Create: `packages/desktop/tests/browser-import-helper-contract.test.mjs`
- Modify: `scripts/language-policy-allowlist.txt`（两个新 `.mjs` 必须入清单；入清单 = 语言政策 spec 批准）

**Interfaces:**
- Produces: `runOracle(args, stdin?, options?) => Promise<{exitCode, stdout, stderr}>`、
  `ORACLE_PATH`（oracle 绝对路径）。

- [x] **Step 1: 写契约断言测试**（已落地）

> 第三条 broker 用例**默认 skip**：broker 模式会触发 UAC 提权与服务通道，必须由人确认，
> 无人值守跑会弹窗并挂住；仅在 `MODE_BROWSER_IMPORT_ORACLE_UAC=1` 时执行（实机验收清单里人工跑）。

```js
// packages/desktop/tests/browser-import-helper-contract.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { runOracle } from "../../../scripts/browser-import-helper-oracle.mjs";

const PROTOCOL = "ZCODE_BROWSER_IMPORT_HELPER";

test("--version 输出 5 字段且协议名/版本冻结", async () => {
  const r = await runOracle(["--version"]);
  assert.equal(r.exitCode, 0);
  const f = r.stdout.trim().split("\t");
  assert.equal(f.length, 5);
  assert.equal(f[0], PROTOCOL);
  assert.equal(f[1], "2");
  assert.match(f[2], /^(x64|arm64)$/);
});

test("无参数与未知参数退出码为 2", async () => {
  assert.equal((await runOracle([])).exitCode, 2);
  assert.equal((await runOracle(["--nope"])).exitCode, 2);
});
```

- [x] **Step 2: 先跑确认失败**（已验：`Cannot find module`）

Run: `node --test packages/desktop/tests/browser-import-helper-contract.test.mjs`
Expected: FAIL —— `Cannot find module '../../../scripts/browser-import-helper-oracle.mjs'`

- [x] **Step 3: 实现 harness**（已落地 `scripts/browser-import-helper-oracle.mjs`）

```js
// scripts/browser-import-helper-oracle.mjs
// 黑盒对拍取数器：只运行旧产物、记录输入输出，不读也不移植其实现（clean-room-policy §黑盒对拍）。
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const ORACLE = join(
  repoRoot, "packages", "desktop", "bundled-tools", "win32-x64",
  "browser-import", "mode-browser-import-helper.exe",
);
const TIMEOUT_MS = 20_000;

export function runOracle(args, stdin) {
  if (!existsSync(ORACLE)) {
    throw new Error(
      `oracle 缺失：${ORACLE}\n先执行 pnpm --filter @mode/desktop prepare:browser-import-helper`,
    );
  }
  return new Promise((resolve, reject) => {
    const child = spawn(ORACLE, args, { stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "", stderr = "";
    const timer = setTimeout(() => { child.kill(); reject(new Error("oracle_timeout")); }, TIMEOUT_MS);
    child.stdout.on("data", (d) => { stdout += d; });
    child.stderr.on("data", (d) => { stderr += d; });
    child.on("error", reject);
    child.on("close", (exitCode) => { clearTimeout(timer); resolve({ exitCode, stdout, stderr }); });
    child.stdin.end(stdin ?? "");
  });
}
```

- [x] **Step 4: 跑测试确认通过**

Run: `node --test packages/desktop/tests/browser-import-helper-contract.test.mjs`
Expected: PASS（2 pass / 1 skip）

- [x] **Step 5: 两个新 `.mjs` 入语言政策豁免清单**

`scripts/language-policy-allowlist.txt` 追加 `scripts/browser-import-helper-oracle.mjs` 与
`packages/desktop/tests/browser-import-helper-contract.test.mjs`（门禁实测：不入清单直接 FAIL）。

- [x] **Step 6: 提交**（已落地）

```bash
git add scripts/browser-import-helper-oracle.mjs packages/desktop/tests/browser-import-helper-contract.test.mjs
git commit -m "test(browser-import): oracle 黑盒取数 harness 与契约断言"
```

---

### Task 2: crate 骨架 + `--version`（契约 A）

**Files:**
- Create: `crates/browser-import-helper/Cargo.toml`
- Create: `crates/browser-import-helper/src/main.rs`
- Create: `crates/browser-import-helper/src/cli.rs`
- Test: `crates/browser-import-helper/tests/cli.rs`

**Interfaces:**
- Produces: `browser_import_helper::cli::parse(args: &[String]) -> Mode`，其中
  `enum Mode { Version, Broker { parent_pid: u32 }, UsageError }`；
  `cli::version_banner() -> String` 返回契约 A 的 5 字段行。

- [ ] **Step 1: 写失败的测试**

```rust
// crates/browser-import-helper/tests/cli.rs
use browser_import_helper::cli::{parse, version_banner, Mode};

#[test]
fn version_banner_has_five_frozen_fields() {
    let f: Vec<&str> = version_banner().trim().split('\t').collect();
    assert_eq!(f.len(), 5);
    assert_eq!(f[0], "ZCODE_BROWSER_IMPORT_HELPER");
    assert_eq!(f[1], "2");
    assert!(f[2] == "x64" || f[2] == "arm64");
}

#[test]
fn parses_version_and_broker() {
    let a = vec!["--version".to_string()];
    assert!(matches!(parse(&a), Mode::Version));
    let b = vec!["--broker".to_string(), "--parent-pid".to_string(), "42".to_string()];
    assert!(matches!(parse(&b), Mode::Broker { parent_pid: 42 }));
}

#[test]
fn unknown_args_are_usage_error() {
    assert!(matches!(parse(&[]), Mode::UsageError));
    assert!(matches!(parse(&["--nope".to_string()]), Mode::UsageError));
}
```

- [x] **Step 2: 跑测试确认失败**（已验：`cargo test` 编译失败，crate 不存在）

Run: `cargo test --manifest-path crates/browser-import-helper/Cargo.toml`
Expected: 编译失败 —— crate 不存在

- [x] **Step 3: 建 crate 与实现**（已落地）

```toml
# crates/browser-import-helper/Cargo.toml
[package]
name = "browser-import-helper"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "mode-browser-import-helper"
path = "src/main.rs"

[lib]
name = "browser_import_helper"
path = "src/lib.rs"

[dependencies]
```

```rust
// crates/browser-import-helper/src/cli.rs
pub const PROTOCOL: &str = "ZCODE_BROWSER_IMPORT_HELPER";
pub const PROTOCOL_VERSION: &str = "2";

#[derive(Debug, PartialEq)]
pub enum Mode {
    Version,
    Broker { parent_pid: u32 },
    UsageError,
}

pub fn parse(args: &[String]) -> Mode {
    match args {
        [a] if a == "--version" => Mode::Version,
        [a, b, c] if a == "--broker" && b == "--parent-pid" => match c.parse() {
            Ok(pid) => Mode::Broker { parent_pid: pid },
            Err(_) => Mode::UsageError,
        },
        _ => Mode::UsageError,
    }
}

pub fn version_banner() -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\n",
        PROTOCOL,
        PROTOCOL_VERSION,
        std::env::consts::ARCH,
        env!("CARGO_PKG_VERSION"),
        option_env!("MODE_GIT_REVISION").unwrap_or("unknown"),
    )
}
```

> `std::env::consts::ARCH` 在 Windows x64 上为 `x86_64`，而契约要求 `x64`：Step 3 落地时
> 用显式映射函数 `arch_token()`（`x86_64 → "x64"`、`aarch64 → "arm64"`），并在 Task 2 的
> 测试断言 `f[2]` 为 `x64`/`arm64` 二者之一——先写这个映射再继续。

```rust
// crates/browser-import-helper/src/main.rs
use browser_import_helper::cli::{parse, version_banner, Mode};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Mode::Version => print!("{}", version_banner()),
        Mode::Broker { .. } => {
            // Task 3/4 实现；未实现前 fail-closed 且只走契约 B 的 stdout 通道。
            eprintln!("not_implemented");
            std::process::exit(2);
        }
        Mode::UsageError => std::process::exit(2),
    }
}
```

- [x] **Step 4: 编译面确认**（本机只能做到这一步）

Run: `cargo check --manifest-path crates/browser-import-helper/Cargo.toml --all-targets`
Run: `cargo clippy --manifest-path crates/browser-import-helper/Cargo.toml --all-targets -- -D warnings`
Expected: 两者均绿（已验证）。**`cargo test` 本机跑不了**——无链接器（见 Global Constraints），
单测改由 CI Windows 腿执行，已接线。

- [ ] **Step 5: 与 oracle 对拍 `--version`**〔待能出产物的环境〕

在 Task 1 的 harness 加 `runRustHelper()` 并列比较：除第 4/5 字段（版本与 git 修订，构建期注入）
外，字段 0/1/2 必须逐字节一致。**阻塞点**：本机产不出 `mode-browser-import-helper.exe`，
需 CI 或装了链接器的机器。

- [x] **Step 6: 提交**（已落地）

```bash
git add crates/browser-import-helper
git commit -m "feat(browser-import): Rust helper 骨架与 --version 契约 A"
```

---

### Task 3: broker wire 协议编解码（契约 B 的纯逻辑面）

**执行记录（2026-10-15）：本任务已落地。** 实况见 `crates/browser-import-helper/src/protocol.rs`
与 `tests/protocol.rs`；下面步骤里的示例代码是落地前草稿，已被实际实现取代。落地中发现并修正了
两处契约细节，均已回写 `docs/specs/browser-import-helper-contract.md`：

1. **令牌是三个不同串**（草稿把源头的 wire 令牌与横幅令牌混成一个）：broker 请求/响应帧用
   `MODE_BROWSER_IMPORT_V1`（宿主 `HELPER_PROTOCOL`），`--version` 横幅第一字段用
   `MODE_BROWSER_IMPORT_HELPER`（宿主 `HELPER_VERSION_PROTOCOL`），协议版本 `2`。**旧 C# 产物对两者
   都发 `ZCODE_*` 品牌，与宿主两条断言都不符——这是本仓库现役功能缺陷**，Rust 版按宿主契约修正。
2. **reason 全集是 15 项**（白名单 13 + 宿主单独识别的 `elevation_cancelled` /
   `helper_verification_failed`），不是草稿写的 13。

另有实测结论：失败**不保证有 ERR 帧**——畸形帧/空 stdin 是静默非零退出（退出码 1），
宿主一律判 `invalid_response`。测试覆盖了 CRLF/缺尾换行、非 UTF-8 路径、未知 reason 归并、
以及「合法帧在 P2 前必须不发帧」的反向锁。

**Files:**
- Create: `crates/browser-import-helper/src/protocol.rs`
- Test: `crates/browser-import-helper/tests/protocol.rs`

**Interfaces:**
- Consumes: `cli::{PROTOCOL, PROTOCOL_VERSION}`
- Produces: `protocol::Request { encrypted_key: Vec<u8>, chrome_exe_path: String }`、
  `protocol::parse_request(line: &str) -> Result<Request, &'static str>`、
  `protocol::ok_response(key: &[u8; 32]) -> String`、`protocol::err_response(reason: &str) -> String`、
  `pub const REASONS: [&str; 15]`（白名单 13 + 宿主单独识别 2）、`HOST_WHITELIST_REASONS`、
  `HOST_SPECIAL_REASONS`、`KEY_LEN`。

- [ ] **Step 1: 写失败的测试**

```rust
// crates/browser-import-helper/tests/protocol.rs
use base64::Engine;
use browser_import_helper::protocol::{err_response, ok_response, parse_request, REASONS};

#[test]
fn parses_contract_b_request() {
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 12]);
    let exe = base64::engine::general_purpose::STANDARD.encode(b"C:\\chrome.exe");
    let line = format!("ZCODE_BROWSER_IMPORT_HELPER\t{key}\t{exe}\n");
    let req = parse_request(&line).unwrap();
    assert_eq!(req.encrypted_key, vec![7u8; 12]);
    assert_eq!(req.chrome_exe_path, "C:\\chrome.exe");
}

#[test]
fn rejects_malformed_frames() {
    assert!(parse_request("").is_err());
    assert!(parse_request("ZCODE_BROWSER_IMPORT_HELPER\tonly\two").is_err());
    assert!(parse_request("WRONG_PROTOCOL\tAA==\tAA==").is_err());
}

#[test]
fn response_frames_are_three_fields() {
    let ok = ok_response(&[1u8; 32]);
    let f: Vec<&str> = ok.trim().split('\t').collect();
    assert_eq!(f, vec!["ZCODE_BROWSER_IMPORT_HELPER", "OK",
        "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE="]);
    let err = err_response("unsupported_key");
    assert_eq!(err.trim(), "ZCODE_BROWSER_IMPORT_HELPER\tERR\tunsupported_key");
}

#[test]
fn reason_set_matches_host_whitelist() {
    assert_eq!(REASONS.len(), 13);
    assert!(REASONS.contains(&"elevation_cancelled"));
    assert!(REASONS.contains(&"helper_verification_failed"));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path crates/browser-import-helper/Cargo.toml --test protocol`
Expected: FAIL —— `unresolved import browser_import_helper::protocol`

- [ ] **Step 3: 实现**

```rust
// crates/browser-import-helper/src/protocol.rs
use base64::Engine;
use crate::cli::PROTOCOL;

pub const REASONS: [&str; 13] = [
    "helper_failed", "broker_initialization_failed", "controller_handshake_failed",
    "service_channel_failed", "elevation_failed", "timeout", "peer_verification_failed",
    "service_failed", "validation_failed", "unsupported_key", "cng_failed",
    "decryption_failed", "service_cleanup_failed",
];

#[derive(Debug, PartialEq)]
pub struct Request { pub encrypted_key: Vec<u8>, pub chrome_exe_path: String }

pub fn parse_request(line: &str) -> Result<Request, &'static str> {
    let f: Vec<&str> = line.trim_end_matches('\n').split('\t').collect();
    if f.len() != 3 || f[0] != PROTOCOL { return Err("validation_failed"); }
    let enc = base64::engine::general_purpose::STANDARD.decode(f[1]).map_err(|_| "validation_failed")?;
    let path = base64::engine::general_purpose::STANDARD.decode(f[2])
        .map_err(|_| "validation_failed")
        .and_then(|b| String::from_utf8(b).map_err(|_| "validation_failed"))?;
    Ok(Request { encrypted_key: enc, chrome_exe_path: path })
}

pub fn ok_response(key: &[u8; 32]) -> String {
    format!("{}\tOK\t{}\n", PROTOCOL, base64::engine::general_purpose::STANDARD.encode(key))
}

pub fn err_response(reason: &str) -> String {
    format!("{}\tERR\t{}\n", PROTOCOL, reason)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path crates/browser-import-helper/Cargo.toml`
Expected: PASS（全部用例）

- [ ] **Step 5: 提交**

```bash
git add crates/browser-import-helper
git commit -m "feat(browser-import): broker wire 协议编解码与 reason 全集"
```

---

## P2：需要真实 Windows 会话的层

### Task 4: 提权与服务通道

**Files:**
- Create: `crates/browser-import-helper/src/elevation.rs`
- Create: `crates/browser-import-helper/src/service_channel.rs`
- Test: `crates/browser-import-helper/tests/elevation.rs`

**Interfaces:**
- Produces: `service_channel::run_broker(req: &Request, parent_pid: u32) -> Result<[u8; 32], &'static str>`，
  错误值必须取自 `protocol::REASONS`。

- [ ] **Step 1: 写失败的测试（可用 mock 的部分）**

只对**可无副作用验证**的部分写单测：`parent_pid` 为 0/不存在的进程时返回 `peer_verification_failed`。

- [ ] **Step 2: 实现**（取材：Microsoft 公开文档）
- 提权：`ShellExecuteExW` + `runas` 谓词；用户取消 → `elevation_cancelled`（**注意这是契约 B 里
  唯一被宿主单独映射的 reason 之一**）。
- 服务通道：一次性的进程间通道；握手失败 → `controller_handshake_failed` / `service_channel_failed`；
  结束清理失败 → `service_cleanup_failed`。
- 对端校验：用 `--parent-pid` 确认父进程身份，不符 → `peer_verification_failed`。

- [ ] **Step 3: 跑测试**

Run: `cargo test --manifest-path crates/browser-import-helper/Cargo.toml`
Expected: PASS（可覆盖部分全绿）

- [ ] **Step 4: 提交**

```bash
git add crates/browser-import-helper
git commit -m "feat(browser-import): 提权与服务通道（fail-closed 到契约 reason）"
```

---

### Task 5: Chrome app-bound 密钥解封

**Files:**
- Create: `crates/browser-import-helper/src/decrypt.rs`
- Test: `crates/browser-import-helper/tests/decrypt.rs`

**Interfaces:**
- Produces: `decrypt::unseal(req: &Request) -> Result<[u8; 32], &'static str>`
  （错误值取 `unsupported_key` / `cng_failed` / `decryption_failed`）。

- [ ] **Step 1: 写失败的测试**：输入长度非法 / 路径不存在 → 对应 reason；成功路径需真实 Chrome 环境（CI skip）。
- [ ] **Step 2: 实现**（取材：Microsoft 公开文档 + Chrome 公开的 app-bound encryption 说明）。

**注意**：契约要求输出**恰好 32 字节**——长度不符必须 `decryption_failed`，宿主会把它当
`invalid_response` 之外的失败路径。

- [ ] **Step 3: 跑测试** Run: `cargo test --manifest-path crates/browser-import-helper/Cargo.toml`
- [ ] **Step 4: 提交** `git commit -m "feat(browser-import): app-bound 密钥解封"`

---

## P3：接线、打包、收尾

### Task 6: 构建脚本与打包接线

**Files:**
- Create: `scripts/build-browser-import-helper.mjs`（cargo 构建 → 复制为 `mode-browser-import-helper.exe`）
- Modify: `packages/desktop/package.json`（`prepare:browser-import-helper` 改指新脚本）
- Modify: `packages/desktop/electron-builder.config.js`（extraResources 保持 `tools/browser-import` 落点）
- Delete: `packages/desktop/scripts/build-windows-browser-import-helper.mjs`
- Modify: `scripts/language-policy-allowlist.txt`（删除 `.../Program.cs` 行）

> 与路线图 Global Constraints 一致：同一窗口期只允许一处改 `electron-builder.config.js`（CUA 一期的
> `tools/cua-helper` 已落地，本任务只加/改 browser-import 一项）。

- [ ] **Step 1: 写构建脚本**（对齐 `scripts/build-cua-helper.mjs` 的形态：探测 cargo → 构建 → 复制 → 校验存在）
- [ ] **Step 2: 跑构建** Run: `pnpm --filter @mode/desktop prepare:browser-import-helper`
  Expected: 产出同路径 exe，且 `--version` 与 oracle 前 3 字段一致
- [ ] **Step 3: 删 C# 与旧脚本，跑语言门禁** Run: `node scripts/check-language-policy.mjs`
  Expected: PASS（C# 行已移除，无残留 .cs）
- [ ] **Step 4: 提交** `git commit -m "build(browser-import): helper 改由 Rust 产出，删 C# 实现"`

---

### Task 7: 实机验收与 CI

**Files:**
- Modify: `.github/workflows/release.yml`（Windows 腿加本 crate 的 `cargo test` / `clippy -D warnings`）
- Modify: `docs/plans/2026-10-09-tauri-rewrite-roadmap.md`（勾掉 M0 任务 6）

- [ ] **Step 1: CI 接线**：与 `crates/mode-cua-ax` 同形（rust-cache 的 `workspaces` 加本 crate）。
- [ ] **Step 2: 实机验收**（**必须真机，人工执行**）：Windows 上装 Chrome，登录并写入 cookie，
  从打包版 app 触发一次「导入浏览器数据」，成功取回 32 字节密钥；再验一次拒绝 UAC →
  宿主报 `chrome_cookie_elevation_cancelled`。
- [ ] **Step 3: 更新路线图并提交**（见计划交付说明）。

---

## 验收（阶段）

1. `cargo test` 与 `cargo clippy -D warnings` 在本 crate 全绿；
2. 契约 A/B 与 oracle 对拍一致（字段 4/5 除外的构建期字段除外）；
3. 语言门禁 PASS 且 `Program.cs` 已删；
4. Windows 实机导入一次成功 + 一次 UAC 取消路径正确（人工，记录在 PR）。

## 已知阻塞与前置

- **本机无 dotnet/csc**：oracle 靠 `csc.exe`（.NET Framework 4.0.30319）产出，已验证可用；
  换机器需重新确认该路径。
- **P2 无法在 CI 覆盖**：UAC 与服务通道需要交互式桌面会话，CI 只能跑 P1（纯逻辑）与构建；
  P2 的验收靠 Task 7 Step 2 的人工实机清单。
- **签名**：包装态下宿主会校验 helper 的 Authenticode 签名与 app 一致；未签名的本地构建
  只走非包装路径（`isPackaged = false`），实机验收要覆盖包装态则需签名构建。
