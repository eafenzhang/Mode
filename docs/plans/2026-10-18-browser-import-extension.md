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
- [ ] **接到导入流程/UI**：启动通道 → 提示用户点击扩展 → 等待 payload → 写 Electron session
      → 返回计数；与现有 helper 路径互斥；错误码映射到 UI。
- [ ] 应用退出/异常时的通道清理（主进程生命周期接线）。
- 不持久化授权；日志只记数量/reason。

### T5：安装与注册

- Native host manifest（`allowed_origins` 只含固定扩展 ID）。
- 注册位置：`HKCU\Software\Google\Chrome\NativeMessagingHosts\com.mode.browser_import`。
- 扩展安装指引/企业策略文档；扩展缺失时的可操作提示。
- 发布态签名/哈希校验（fail-closed）。

### T6：验收与评审

- 按 spec §8 的 6 条场景逐条跑；记录原始输出。
- 安全评审 R1–R10；多引擎 AV/EDR（若涉及 host 注入/提权，本方案不需要）。
- 日志/错误路径不含 cookie 值与 token。

### T7：收尾

- 发版说明与回滚（保留现有 helper/C# 直到 P1 稳定）。
- 决定是否删除 app-bound helper/C#（开放决策）。

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
