# 计划：Chrome 扩展导入（P1）Implementation Plan

配套 spec：`docs/specs/chrome-browser-import-extension.md`。
安全门禁：`docs/specs/chrome-abe-import-security.md`（R1–R10）。
依据：`docs/plans/2026-10-17-chrome-abe-import-spike.md` 的 T1 最终结论（Medium 完整性下
扩展读到 34/34 条 v20；P1 成立）。

## 决策默认项（可推翻）

1. **传输用 Native Messaging**（Chrome 受支持），扩展在用户点击时读取 cookie；不用注入。
2. **Native host 用 Rust**（与仓库语言政策一致），单次进程、不落盘。
3. **host ↔ app 用命名管道 + 一次性 token**；token 由 Mode main 生成，写入当前用户可读的
   `channel.json`，导入完成即删除。
4. **分发先走侧载/企业策略**，Web Store 作为开放决策；扩展源码用 TypeScript，构建产物 JS
   放在 gitignore 的 `dist/`（语言政策只约束入库源码）。
5. **先只导入 cookie**；LocalStorage 后置。

## 任务

### T0：spec（已完成，2026-10-18）

`docs/specs/chrome-browser-import-extension.md`。

### T1：Native Messaging 协议层（本轮）

- [x] 新建 `crates/browser-import-host`：4 字节小端长度 + UTF-8 JSON 的读写、1 MiB 上限、
      请求形状校验、稳定错误码。
- [x] 单测：往返、长度非法、超限、非法 JSON、缺字段/类型错、错误响应。
- [x] 接入根 workspace；`cargo test -p browser-import-host` + clippy 全绿。
- [x] CI 接线：`release.yml` 的 `cargo test --locked` 加入 `-p browser-import-host`
      （clippy 的 `--workspace` 已覆盖）。
- 执行记录：`cargo test -p browser-import-host` **9 passed**；`clippy -D warnings` 零告警；
  `rustfmt --check` 干净。二进制目前只做「解析 + 回计数」，host ↔ app 管道是 T2。

### T2：host ↔ app 命名管道

- [x] **host 侧**：读取 `channel.json`、校验版本/管道名/token/TTL、连接命名管道、
      `hello`+token → `ready` → 原样发送已校验 payload → 读 `count`；失败码映射到 spec §5。
      实现 `crates/browser-import-host/src/channel.rs`（`ChannelInfo` / `deliver` / `handshake`）。
- [x] 单测 5 条：hello/payload 帧、token 不匹配、非法 ready、缺 count、过期/非法管道/短 token。
- [ ] **Mode main 侧**（T4）：生成管道名 + token + `channel.json`（ACL 当前用户）、握手服务、
      TTL 清理；本 crate 只实现 host 侧。
- [ ] Windows 命名管道的真机 E2E（T6）。

### T3：扩展（TS 源码）

- [x] 新增 `packages/browser-import-extension`：MV3 manifest（`cookies` + `nativeMessaging`）、
      popup（用户点击 → `chrome.cookies.getAll` → `sendNativeMessage`）、最小 `chrome` 类型声明。
- [x] 构建：`node --import tsx scripts/build.ts`（tsc → `dist/` + 复制 manifest/html；
      `dist/` 已被根 `.gitignore` 忽略）；源码只含 TS/HTML/JSON。
- [x] `pnpm --filter @mode/browser-import-extension typecheck` 与 `build` 均通过。
- [ ] 固定扩展 ID（manifest `key`）与 Native host `allowed_origins`：归 T5。
- [ ] 真机验收（Medium 完整性、Chrome 155、34 条 v20）：归 T6。

### T4：Mode main 接线

- [x] **接收器**：`packages/desktop/src/main/browserImportChannel.ts`——生成命名管道 + 一次性
      token + `channel.json`（TTL ≤60s）、帧读写、token 常量时间比较、payload 校验、
      `onCookies` 回调、TTL 关闭与 channel.json 清理；日志只记数量/reason。
- [x] 单测 4 条（Windows）：happy path、token 不匹配、非法 payload、TTL 清理。
- [x] **接到导入流程/UI**：启动通道 → 提示用户点击扩展 → 等待 payload → 写 Electron session
      → 返回计数；与现有 helper 路径互斥；错误码映射到 UI。
- [x] 应用退出/异常时的通道清理（主进程生命周期接线）。
- 执行记录（2026-10-18）：
  - `browserImportChannel` 新增 `onError` 上报终止码（`channel_timeout` / `channel_token_mismatch` /
    `payload_too_large` / `invalid_payload` / `cookie_write_failed`）；socket/server `error`
    不再升级为未捕获异常；TTL 到期主动通知调用方。
  - `browserImportExtension` 改为快速失败并返回 spec §5 稳定码；成功/失败计数改为真实
    `skipped`/`failed`；新增 `closeActiveBrowserImportChannels()`，`will-quit` 时同步删除
    `channel.json`。
  - 设置页 Windows dev 构建新增「通过 Chrome 扩展导入（实验）」入口，等待期间显示点击扩展
    提示；zh/en/fa 文案补齐；`shared` 错误码联合类型与 UI 映射同步。
  - 新增取消链路：扩展 popup `pagehide` 尽力发送 `{v:1,kind:"cancel"}` → Rust host 转发 →
    通道 `onCancel` → `import_cancelled`；主流程仍以 TTL 兜底。
  - 新增 Native host 预检：清单存在且 `path` 指向真实可执行文件，否则导入前直接返回
    `native_host_missing`。
  - 无连接 TTL 与已连接卡住区分：前者 `extension_missing`，后者 `channel_timeout`。
  - 验证：`tsx --test` 16 条通过；`cargo test -p browser-import-host` 16 条通过；
    `pnpm typecheck` 0；改动文件 oxlint 0；架构检查 0 违规。
- 不持久化授权；日志只记数量/reason。

### T5：安装与注册

- [x] 固定扩展 ID：`src/manifest.json` 写入 2048-bit RSA 公钥 `key`；ID =
      `bfgflaoohgcpcjkmefbddilhlmbbmoic`。
- [x] Native host manifest 模板（`native-host/com.mode.browser_import.json`）+
      注册/注销脚本 `scripts/register-native-host.ts`（写 `%LOCALAPPDATA%` 清单 + HKCU 注册表）。
- [x] 实测注册成功：`reg query HKCU\Software\Google\Chrome\NativeMessagingHosts\com.mode.browser_import`
      指向 `%LOCALAPPDATA%\Mode\browser-import\native-host\com.mode.browser_import.json`；
      host 为 `target/release/mode-browser-import-host.exe`。
- [x] README 更新安装/注册步骤；注销命令已提供。
- [x] 分发（Web Store / 企业策略 / 侧载）与发布态 Authenticode 签名：决策记录见 T7。
- [x] 扩展缺失时的可操作提示（UI）：`extension_missing` / `native_host_missing` 均有独立文案；
      实测两条路径都能在 60s 内或立即给出可操作错误。

### T6：验收与评审

- [x] **Native host 二进制 → 命名管道 → 接收端 E2E**（不依赖 Chrome UI）：
      2 条 cookie → host 解析 2、接收端 `SERVER_COUNT 2`、host 回 `{ok:true,count:2}`、exit 0。
- [x] **Chrome 扩展 → Native host → Mode main 写入的完整 E2E**：独立测试 profile（Chrome 155、
      `Extensions.loadUnpacked` 加载 `packages/browser-import-extension/dist`）+ 3 条测试 cookie
      → 扩展 popup 显示「已导入 6 条」→ main 返回
      `{success:true,cookies:{imported:6,skipped:0,failed:0}}`，`channel.json` 导入后即删除。
      （耗时约 6.5s；profile 内另有 3 条 Chrome 自带 cookie。）
- [x] 按 spec §8 的 6 条场景逐条跑；记录原始输出。
      - §8.1 真实 profile（v20）：**自动化环境跑不了**。真实 Chrome profile 当前是 0 条 cookie；
        有数据的 Edge profile 是 489 条全 v20，但 app-bound 解密要求 Medium 完整性且该启动方式
        下 Edge 不监听 CDP 端口，无法在无人值守下驱动扩展。v20 可读性仍以 spike 的 34/34 结论为准，
        最终验收留给交互式 Medium 会话。
      - §8.2 取消/关闭 popup：**实测通过**——popup 触发 `pagehide` 后 Mode 返回
        `{success:false,error:"import_cancelled"}`、未写入任何 cookie、`channel.json` 已删除。
      - §8.3 扩展未安装 / native host 未注册：**实测通过**——移除 host 清单后立即返回
        `native_host_missing`；不连接扩展等待 60s 返回 `extension_missing`。
      - §8.4 payload > 1 MiB：单测覆盖 `payload_too_large`（通道与扩展双侧都会拒绝）。
      - §8.5 无持久授权：逐次 per-call 开关，未写任何授权状态。
      - §8.6 日志不含值/token：实测主日志与 dev console 均为 0 命中。
- [x] 安全评审 R1–R10（结论见下节；R5/R10 仍有发布前开放项）。
- [x] 日志/错误路径不含 cookie 值与 token（实测：`e2e_a/e2e_b/e2e_c`、测试 token 在主日志与
      dev console 均 0 命中）。

### T7：收尾

- [x] 发版说明与回滚（保留现有 helper/C# 直到 P1 稳定）。
- [x] 决定是否删除 app-bound helper/C#：**暂不删除**，P1 至少稳定一个发布周期后再评估。
- 决策记录（2026-10-18，按「自动执行」授权拍板）：
  - **D1 分发**：P1 走侧载 / 企业策略，不做 Web Store；扩展仍固定 ID + 公开 key。
  - **D2 签名门禁**：发布前必须对 `mode-browser-import-host.exe` 做 Authenticode、对扩展包做
    固定 ID 校验；在签名与分发落地前，设置页入口只出现在 Windows dev 构建（R5 的补偿控制）。
  - **D3 helper/C#**：保留，回滚路径 = 关闭扩展入口（per-call 开关默认关闭）。
  - **D4 取消语义**：`pagehide` 为尽力而为；主流程 TTL 兜底，不把「必达」作为正确性前提。
  - **D5 v20 验收**：只能在交互式 Medium 完整性会话里做；自动化 runbook 明确标注该限制，
    不把「0 条 cookie」误判为链路失败。

## 安全评审（R1–R10，2026-10-18）

| # | 结论 | 证据 / 残余风险 |
| --- | --- | --- |
| R1 逐次授权 | 通过 | 每次调用只带本次 per-call 开关，无持久授权；通道一次性 token + TTL；重放命中 `channel_token_mismatch`（单测）。 |
| R2 仅当前用户 profile | 通过 | 扩展走 `chrome.cookies.getAll`，main 不接受外部 profile 路径。 |
| R3 数据最小化 | 通过 | popup 只映射 9 个 cookie 字段；不读密码、历史、支付数据。 |
| R4 不落盘敏感物 | 通过 | cookie 值只在内存；`channel.json` 只含 pipe/token/TTL；日志只有数量/reason；实测 0 命中。 |
| R5 完整性 | **未满足（发布前置）** | dev 路径使用本地构建的 host 与未打包扩展，无签名/哈希校验；补偿控制见 D2：入口只在 Windows dev 构建暴露，发布前必须补 Authenticode 与扩展分发校验。 |
| R6 最小权限 | 通过 | 无 UAC、无常驻服务；host 为单次进程。 |
| R7 受支持机制优先 | 通过 | 扩展 + Native Messaging，无注入。 |
| R8 可审计 | 通过 | 只记 count/reason/expiresAt；设置页展示上次导入结果。 |
| R9 可关闭/可回滚 | 通过 | 默认 helper 路径不变；per-call 开关 + env 开关；UI 仅在 dev 构建出现。 |
| R10 合规 | **已拍板（D1）** | P1 走侧载 / 企业策略，不做 Web Store；企业可继续用策略禁用扩展或 Native host。 |

- 多引擎 AV/EDR：本方案无注入/提权，按安全 spec §5 可免除；发布前如仍要求扫描，只扫
  `mode-browser-import-host.exe`。
- 打开项：R5（签名/哈希）、R10（分发与策略）、§8.3（扩展/host 缺失检测与 UI 指引）、
  `import_cancelled`（扩展关闭信号）。

## 门禁

- `cargo test -p browser-import-host` + `clippy -D warnings` + `rustfmt --check`；
- `pnpm typecheck` / `pnpm lint` / `pnpm architecture:check --changed`；
- 实机 E2E 在 Medium 完整性会话（Chrome 155）。

## 风险

| 风险 | 对冲 |
| --- | --- |
| 扩展分发受限（企业策略/商店） | 先侧载；文档化企业部署；Web Store 作为开放决策 |
| Native Messaging 1 MiB 上限 | 分片；或先只支持常见 profile 大小并给出明确错误 |
| 管道 token 泄露 | 一次性、≤60s、仅当前用户 ACL、导入后删除 |
| 与现有 helper 路径并存造成双写 | 单次导入只走一条路径；UI 明示当前路径 |
| 扩展权限过宽 | 只申请 cookies/nativeMessaging；host_permissions 按需评估 |
