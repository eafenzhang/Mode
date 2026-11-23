# Spec：`cua-permission` 迁移可行性（LT-16.5 前置评估）

状态：**评估完成，结论为「不可按频道直迁，暂缓」**（2026-11-23）。本文件记录证据与
可选路径，供后续决策；不包含实现。

## 0. 结论

`cua-permission` 的三个方法里：

- `getStatus`：**部分可行**——Rust 可经既有 broker socket 调 `permission_status`，
  但「按需拉起 helper」「独立进程抓屏探针」「生命周期货币性校验」都在 Node 侧；
- `restartHelper`：**不可行（首切）**——依赖托管式 helper 生命周期（`resolver.restart()` /
  `restartAfterPermissionGrant()`），这套状态机在 Node 侧且与 prebuilt `.app` 强耦合；
- `openPermissionOnboarding`：**不是 host 频道的事**——它是 host→main 透传、由壳开原生窗口。

因此不能像 `provider-settings` / `remote-desktop` 那样「把频道搬进 Rust」；硬搬会造出
壳内频道是整频道切换、而 `restartHelper`/onboarding 只有 Node 能做的**半个频道**，
违反 `host-ownership-switch.md` §5 与 AGENTS.md 的整频道约束。

## 1. 证据

### 1.1 平台面已经是 macOS-only

`packages/services/src/node.ts`：

```ts
export function shouldUseCuaPermissionService(opts: { platform?; cuaEnabled: boolean }): boolean {
  return (opts.platform ?? process.platform) === "darwin" && opts.cuaEnabled;
}
```

非 darwin 时 `getStatus` 直接返回 `{available: false, reason: "CUA permissions are only
available on macOS."}`，`restartHelper` 返回 `{ok:false, ...}`。也就是说：**Windows/Linux
上这个频道本来就只有「不可用」一种结果**，迁移它不产生任何产品价值。

### 1.2 权限真值来自一个 prebuilt `.app`，仓库里没有它的源码

- `HELPER_APP_NAME = "Mode Computer Use.app"`，bundle id `dev.mode.cua-helper`
  （`packages/mode-cua/broker-helper-constants.js`）。
- 路径解析：`resolveBundledCuaHelperAppPath()` → `resourcesPath/cua-helper/Mode Computer Use.app`
  （或 `MODE_CUA_BUNDLED_HELPER_APP_PATH` 注入）。
- 仓库内的构建脚本 `scripts/build-cua-helper.mjs` **只构建 Windows 的 napi addon**
  （`crates/mode-cua-ax` → `mode_cua_ax.dll` → `cua_ax.node` + `entry.cjs` +
  `runtime-manifest.json`）；`crates/mode-cua-ax/Cargo.toml` 的依赖全是 `windows` crate。
- 全仓 `*.swift` / `*.pbxproj` 检索为 **0 个文件**。

推论：macOS 侧 TCC（Accessibility / Screen Recording）查询与 AX 交互都在那个不透明的
`.app` 里，Rust 侧无法复刻，只能（a）重写成 macOS 原生 Rust（objc2 + 自行申请 TCC，
风险与工作量都远超一个频道）或（b）继续经它的 socket 调用。

### 1.3 `permission_status` 不在 broker 方法表内

`crates/cua-raw-input/src/broker_protocol.rs`：

```rust
pub const BROKER_METHODS: [&str; 12] = [
    "health", "list_apps", "list_windows", "observe", "capture", "perform",
    "raw_input", "launch_app", "screen_probe",
    "rd_media_start", "rd_media_switch", "rd_media_stop",
];
```

`dispatch_request` 对表外方法一律 `method_not_found`。这张表是**服务端**（helper 侧）
的白名单，用来对齐 Windows 路径的 `packages/mode-cua/broker.js`；而 `callBrokerMethod`
（客户端侧）不做白名单检查。所以从 Rust 侧**可以**发出 `permission_status`，
但那是发给 macOS 那个 prebuilt helper 的、仓库里没有对应 TS backend 的方法
（`packages/mode-cua/helper/server.mjs` 的 `createBackend` 同样没有它）。

### 1.4 `getStatus` 的真正复杂度在「拉起」而不是「读取」

`node.ts` 的 `getStatus` 流程（简化）：

1. `shouldUseCuaPermissionService` 门禁（darwin + CUA 启用）；
2. 取当前托管 helper `peek()` 并校验 `isDefaultCuaProductHelperCurrent`；
3. **helper 不在跑** → `probeStableCuaHelperSocket()`（300ms ping）→ 仍未起就
   `launchStandaloneCuaHelperForStatus()`（拉起 standalone helper，无 launcher-pid、
   300s 空闲自休眠）；
4. 有托管 host 时 `host.queryPermissionStatus()`；否则经 socket 调 `permission_status`；
5. `resolveCuaScreenRecordingState()`——**为拿真实 Screen Recording 值必须起新进程**
   （macOS 对已运行进程撤销授权不生效，旧进程会一直报 granted）；
6. `runCuaScreenCaptureReadinessProbe()`——真实抓一帧验像素非空；
7. 每一步后都重新校验 `isDefaultCuaProductHelperCurrent`（生命周期被 dispose 就 fail-closed）。

其中 2/3/5/6/7 全部依赖托管 helper 生命周期与 `.app` 进程管理，无法在「只加一个
host-channels 频道」的范围里完成。

### 1.5 被控端 helper 的创建还有一层准入

`shouldCreateDefaultCuaProductHelper` 要求
`(hasBuiltInCuaPlugin || isRemoteDesktopServerEnabled()) && serviceAuthorityMode === "desktop-local"
&& !hasRemoteWorkspaceIdentity && !hasInjectedResolver`。
这意味着 CUA helper 与**远程桌面被控开关**共享同一套 helper 准入——`remote-desktop`
首切时我刻意留的会话端口，正是这条链路的一部分。两者要么一起迁，要么都不迁。

## 2. 可选路径

| 路径 | 内容 | 代价 | 判断 |
| --- | --- | --- | --- |
| **A. 暂缓，先做别的 live 面** | 保持 Node 提供 `cua-permission`；先迁 `bots` 或 `remote-v3` | 0（仅文档） | **推荐** |
| B. 只迁只读 `getStatus` | Rust 经 socket 调 `permission_status`，无 socket 时返回 `idle` | 中（仍是半个频道：restart/onboarding 留 Node） | 不推荐：违反整频道约束，且非 darwin 无收益 |
| C. Rust 自研 macOS 权限面 | objc2 + TCC 申请 + AX 交互 + 抓屏探针，替代 `.app` | 很高（要重写 prebuilt helper，含签名/公证） | 不推荐：超出 LT-16 范围，且要重新过安全评审 |
| D. 等 remote-desktop 会话端口落地后一起迁 | helper 准入两处共用，届时一次性搬 helper 生命周期 | 高但与 16.4b 续段合并 | 中期正解 |

**建议**：走 A，把 C 列为「若将来要摆脱 prebuilt `.app` 才做」的独立长任务；
`cua-permission` 在 16.5 里标记为「与 remote-desktop 会话端口同批」，而不是独立频道。

## 3. 对 LT-16 的影响

- 剩余 live 面重新排序：`bots` → `remote-v3` → `remote` workspaces；
  `cua-permission` 并入 16.4b 续段（`RemoteDesktopPort` 生产实现），因为它和
  `remote-desktop` 共用 helper 准入与 socket 生命周期。
- 16.6 删除窗口的前置相应调整：`cua-permission` 的 Node 实现要等 helper 生命周期
  整体进 Rust 才能删，不单独排期。

## 4. 未验证项（如实标注）

- 本评估基于源码检索与 TS 调用链阅读，**未在 macOS 实机验证** `permission_status`
  在 prebuilt helper 上的真实响应形状；标注为待验证（若走 B 或 C 必须先补这一条）。
- Windows 上 CUA 走 `crates/mode-cua-ax` napi addon（已有 Rust 实现），但
  `ICuaPermissionService` 的权限面在 Windows 恒为 unavailable，故不在迁移范围。
