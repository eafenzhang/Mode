# Spec：OrcaRouter 服务（Rust，分阶段迁移）

状态：2026-11-23 阶段 1（凭据存储）；连接/目录/overlay/频道待后续。

## 背景

`IOrcaRouterService`（`packages/services/src/orcarouter/`）是 OrcaRouter
第三方路由器的设置面：手填 API Key 或 PKCE 登录 → 凭据落项目加密凭据库 →
把同一把 key 对齐进 OrcaRouter provider 的推理配置（`access.apiKey`）→ 目录
按能力/模态过滤。总约 1.3k 行 TS。

本 crate 分阶段迁移：**阶段 1 凭据存储**、**阶段 2 PKCE 连接控制**已落；后续
阶段：`catalog`（208 行）→ `provider_overlay`（94 行）→ 频道与装配。

## 1. 阶段 1：凭据存储（本 spec 覆盖）

物理 key：`provider:orcarouter:api-key`（与 TS 同值），值写入项目
`CredentialStore`（`~/.mode/v2/credentials.json`），不新建密钥库。

| Rust 方法 | TS 方法 | 语义 |
| --- | --- | --- |
| `read` | `read` | 解析持久化记录；损坏/空 apiKey 视为未连接 |
| `load_usable` | `loadUsable` | `needsReauth` 的记录不返回（避免继续用死凭据） |
| `save` | `save` | `apiKey.trim()`；`generation = 旧值 + 1`（旧异步结果不得覆盖新登录）；清除旧 `needsReauth` |
| `clear` | `clear` | 删除凭据 key |
| `status` | `status` | `connected = !needsReauth`；`masked` 只保留前 6 + 末 4；空值空串 |
| `mark_needs_reauth` | `markNeedsReauth` | 只匹配传入的 `generation`/`accountId`；已标记同 generation 幂等返回 true |

辅助函数：`mask_orca_secret`（≤8 全遮）、`looks_like_orca_api_key`
（`^sk-orca-[A-Za-z0-9._-]{6,}$`，仅格式提示，不是有效性证明）。

持久化记录字段：`apiKey/source/grantedScope?/accountId?/generation/needsReauth?/
needsReauthGeneration?`；`source` 只接受 `api-key|pkce`（其它按 `api-key` 归一）。

## 2. 边界

- 阶段 1 不接网络、不写 provider overlay、不注册频道；行为零变化（仅 crate + 测试）。
- 凭据只经 `CredentialStore` 读写，错误只返回文本，不打印密钥。
- 后续阶段必须复用本存储，不另建第二份凭据事实。

## 3. 阶段 2：连接控制（2026-11-23 已落）

`origins`（`origins.rs`）：
- `normalize_orca_origin`（远端强制 HTTPS、HTTP 仅 loopback、去 query/hash/尾斜杠、
  拒绝 userinfo）、`resolve_orca_origins`（显式 AUTH/API > `ORCA_BASE_URL` > 官方默认，
  逐字段回退）、`build_orca_authorize_url`（OOB + S256，参数插入序同 TS）、
  `build_orca_v1_base` / `build_orca_models_url`。

`connect`（`connect.rs`）：
- PKCE：verifier = base64url(random 32B)、challenge = base64url(sha256(verifier))、
  state = base64url(random 16B)；每次 begin 重新生成，verifier 只驻内存；
  随机源不可用时授权失败（不降级）。
- 状态机 `idle/waiting/exchanging/connected/error`，`sessionId/authorizeUrl/hint/
  error/busy/generation` 对外可见；`matches_state` 恒定时间比较。
- `submit_code`：POST `{authBase}/api/v1/auth/keys`（body `code/code_verifier/
  code_challenge_method=S256`），30s 超时；403/400→exchange-rejected、
  429→rate-limited、超时/timeout、其它→network；响应必须带 `key` 且
  `scope == "api"`（不足→scope-downgraded，不把请求 scope 当已授权）。
- generation 守卫：请求前/响应后/持久化前三处校验，旧交换不得覆盖新登录；
  `cancel`/`invalidate_for_page_hide` 递增 generation，`reset` 不递增。
- 成功时用阶段 1 的 store 保存 `source=pkce + grantedScope + accountId`。

## 4. 验收

- `cargo test -p orcarouter-service`（13 条）：阶段 1 全部 + origins 归一/优先级/
  授权 URL、PKCE 确定性、state 比较、begin/cancel generation、真实本地 HTTP 交换
  （成功/403/scope 降级）、cancel 取代在途交换。
- `cargo clippy -p orcarouter-service --all-targets -- -D warnings`。
