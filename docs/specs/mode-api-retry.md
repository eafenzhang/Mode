# Spec：网络重试状态（Rust `mode-api-retry`）

状态：2026-11-02 首版。
真源：`packages/shared/src/mode-api-retry-status.ts`。

## 1. 范围

| Rust 入口                                      | TS 对应                                     |
| ---------------------------------------------- | ------------------------------------------- |
| `normalize_mode_api_retry_status`              | `normalizeModeApiRetryStatus`               |
| `mode_api_retry_from_stream_recovery_payload`  | `modeApiRetryFromStreamRecoveryPayload`     |
| `mode_api_retry_from_model_network_status_payload` | `modeApiRetryFromModelNetworkStatusPayload` |
| `is_mode_model_retry_recovery_progress_payload` | `isModeModelRetryRecoveryProgressPayload`  |
| `SessionApiRetryTracker`（event/snapshot 投影） | `createModeSessionApiRetryRuntimeTracker` |
| `DeferredDraftRegistry`                        | `createModeDeferredDraftRegistry`           |

`ApiRetryOutcome` 区分 `Undefined`（未知/不修改）、`Null`（明确清空）与 `Status`；
状态字段与 TS 同名（kind/attempt/maxRetries/retryDelayMs/errorStatus/error）。

## 2. 归一化规则

- `null` → `Null`；空对象/数组/标量/`undefined` → `Undefined`。
- attempt：正整数 `attempt`，否则 `max(nextAttempt ?? 2 - 1, 1)`；
- maxRetries：`max(maxRetries ?? (maxAttempts ?? attempt+1) - 1, attempt)`；
- retryDelayMs：`retryDelayMs ?? delayMs ?? 0`（非负整数）；
- errorStatus：`errorStatus ?? statusCode ?? null`（非负整数）；
- error：`error ?? message ?? reason ?? "Model retry scheduled"`。
- 非整数/负数按缺失回退。

## 3. 事件映射

- stream recovery：`retryNumber` 正整数才有状态；attempt=`retryNumber`，
  maxRetries=`max(maxRetries ?? attempt, attempt)`，delay 0、errorStatus null，
  error=`message ?? "Model stream recovery retry started"`。
- model network status：
  - `model_retry_scheduled` → 归一化整个 payload；
  - `model_request_started` → 先看 `streamRecovery`；否则 `attempt <= 1` → Null，
    更大 attempt → Undefined（等首个模型进展再清）；
  - `model_request_completed` → Null；
  - `model_request_failed` 且 `retryable !== true` → Null；
  - 其它 → Undefined。
- 恢复进展：`text_delta`/`reasoning_delta` 看非空 `delta`；
  其它 kind 需要非空 `toolCallId`，`tool_input_start/end/tool_call` 为 true，
  `tool_input_delta` 看非空 `delta`。

## 3.1 会话级投影与草稿登记

- key：`(workspaceIdentity.trim() || workspacePath) + "\0" + sessionId`。
- `session.updated` / `streamRecovery.updated`：payload 依次取 `apiRetry`、
  `runtime.apiRetry`、`_meta.mode.apiRetry`，再按**整个 payload**做 stream recovery 与
  model network 映射；得到状态/显式 null 时写入缓存；得到 undefined 且已有重试态时，
  只有「模型恢复进展」payload 才清空。
- snapshot：无 sessionId/workspacePath 原样返回；runtime.apiRetry 已存在时仅记录并返回；
  `completed`/`error` 清空并写 `apiRetry: null`；缓存命中时把状态补进 runtime。
- 草稿登记：remember 使用 snapshot 的 workspace identity，缺省回退 params；
  has/forget 用同一 key 规则（identity trim 优先）。

## 4. 已知差异

1. TS 的 `undefined` 在 Rust 用 `Option::None` 表示；`null` 用 `Some(Value::Null)`。
2. 输出 `ApiRetryStatus` 用 serde 序列化（字段顺序无关，值为整数/字符串/null）。
3. tracker/snapshot 以 JSON 形状处理（保持未知字段与原始顺序无关）；TS 的类型投影由调用方负责。

## 5. 验收

- `pnpm mode-api-retry:fixtures` → `crates/mode-api-retry/fixtures/mode-api-retry.json`
  （14 条 normalize、8 条 stream、9 条 network、10 条 progress）；
  `pnpm mode-api-retry-session:fixtures` → `mode-api-retry-session.json`
  （13 条 tracker 序列、4 条 draft 序列）。
- `cargo test -p mode-api-retry`。
- `cargo clippy -p mode-api-retry --all-targets -- -D warnings`。
