# Spec：子进程环境键策略（Rust `runtime-env`）

状态：2026-11-01 第二版（增加 agent spawn 环境补丁）。
真源：`packages/shared/src/runtimeEnv.ts`、
`packages/services/src/runtime-tools/agentProxyEnv.ts`。

## 1. 范围

| Rust 入口                                        | TS 对应                                  |
| ------------------------------------------------ | ---------------------------------------- |
| `should_sanitize_mode_runtime_env_key`           | `shouldSanitizeModeRuntimeEnvKey`        |
| `should_capture_mode_tool_env_passthrough_key`   | `shouldCaptureModeToolEnvPassthroughKey` |
| `normalize_mode_runtime_env` / `resolve_mode_runtime_env` | 同名函数                        |
| `is_cua_dev_mode_requested`                      | `isCuaDevModeRequested`                  |
| `is_mode_cua_internal_feature_enabled`           | `isModeCuaInternalFeatureEnabled`        |
| `RuntimeEnvContext::sanitize` / `sanitize_in_place` | `sanitizeModeRuntimeEnv` / `...InPlace` |
| `RuntimeEnvContext::captured_credentials`        | `getCapturedModeCuaBrokerCredentials`    |
| `read_mode_tool_env_passthrough_env`             | `readModeToolEnvPassthroughEnv`          |
| `build_mode_tool_env_passthrough_env`            | `buildModeToolEnvPassthroughEnv`         |
| `build_agent_runtime_env`                        | `buildAgentRuntimeEnv`                   |
| `build_agent_endpoint_origin_env`                | `buildAgentEndpointOriginEnv`            |
| `build_agent_workspace_identity_env`             | `buildAgentWorkspaceIdentityEnv`         |
| `is_effective_development_node_env`              | `isEffectiveDevelopmentNodeEnv`          |

`EnvMap = IndexMap<String, String>`（存在即非 undefined；顺序按传入/插入）。
CUA 凭据捕获状态放在 `RuntimeEnvContext`，避免测试并行共享模块级变量。

## 2. 脱敏与透传规则

- 脱敏键：代理/CA（HTTP_PROXY、HTTPS_PROXY、ALL_PROXY、NO_PROXY、NODE_EXTRA_CA_CERTS、
  SSL_CERT_FILE/DIR、REQUESTS_CA_BUNDLE、CURL_CA_BUNDLE、GIT_SSL_CAINFO）、NODE_ENV 等
  运行时键、CUA broker 凭据（socket/authority/refresh marker/旧 token）、全部
  `OTEL_*` 与 `MODE_TELEMETRY_*` 旧遥测键，以及
  `^(npm_config|yarn|pnpm)_(http_proxy|https_proxy|proxy|all_proxy|no_proxy|cafile|ca)$`
  （大小写不敏感）。
- tool-env 透传只捕获「会被脱敏」的键；遥测键与
  `NODE_ENV/ELECTRON_RUN_AS_NODE/NODE_NO_WARNINGS` 及 CUA/远端网络凭据永不透传。
- 已有 `MODE_TOOL_ENV_PASSTHROUGH_JSON` 会先读取（仅接受合法标识符键与字符串值），
  再与环境里的新值合并；输出按 key 排序编码为紧凑 JSON，空集合不产生键。
- CUA 凭据：socket 与 pluginAuthority 同时非空才构成有效组；半组（或只缺其一）清空旧
  快照并 fail-closed；refresh marker 非空才附加。

## 3. 运行时环境判定

- `MODE_RUNTIME_ENV`：trim + 小写后仅接受 `development|production|test`；
  `resolve` 缺省/非法回退 `production`（或调用方给定 fallback）。
- `MODE_CUA_DEV_MODE`：`1/true/on`（大小写不敏感、可带空白）视为开启。
- 内部 CUA 功能：DEV_MODE 开启 → true；`MODE_CUA_PRODUCT_HELPER` 为 `0/false/off` →
  false；其余（含缺省）→ true。

## 3.1 Agent spawn 环境补丁

- 代理：trim 后非空才注入；已带 `scheme://`（大小写不敏感）原样保留，裸
  `host:port` 补 `http://`；同时写 `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`MODE_HTTP_PROXY`。
- No Proxy：按逗号拆分、trim、去空后重新以逗号连接；写
  `NO_PROXY`/`no_proxy`/`MODE_NO_PROXY`。
- 自定义 CA：trim 后非空才写 `NODE_EXTRA_CA_CERTS` 与 `MODE_AGENT_CA_CERT`。
- 端点 origin：trim 后非空写入 `MODE_BASE_URL`。
- workspace identity：trim 后非空写入 `MODE_WORKSPACE_IDENTITY`，本地 workspace 保持
  path fallback（不注入）。
- 键序：代理组 → No Proxy 组 → CA 组，与 TS 返回对象一致。

## 3.2 Node 环境判定

`is_effective_development_node_env` 只认 `MODE_RUNTIME_ENV=development`；`test` 与缺省按
production 处理，用户 shell 的 `NODE_ENV` 不参与判定。

## 4. 已知差异

1. tool-env JSON 的键排序：TS 用 `localeCompare`，Rust 用字节序；值都是 ASCII 环境键，
   消费方按 JSON 对象读取，键序无语义（夹具按解析后的对象比较）。
2. `sanitize` 的返回顺序保留输入顺序；`build` 的输出 JSON 固定排序。
3. TS 的 `undefined` 值与 Rust 的「键不存在」等价；空字符串仍然保留/参与判定。

## 5. 验收

- `pnpm runtime-env:fixtures` → `crates/runtime-env/fixtures/runtime-env.json`
  （5 组 normalize、5 组 resolve、9 组 CUA 开关、4 组 sanitize + inPlace + captured、
  2 条跨步骤凭据序列、19 个键判定、5 组 passthrough 读取、3 组 passthrough 构建、
  9 组 agent 代理/CA 组合、5 组端点 origin、4 组 workspace identity、6 组 Node 环境判定）。
- `cargo test -p runtime-env`（逐项深比较）。
- `cargo clippy -p runtime-env --all-targets -- -D warnings`。
