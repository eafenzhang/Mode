/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明重试矩阵，拆分会降低可核对性。 */
/**
 * 生成 `mode-api-retry` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/shared/src/mode-api-retry-status.ts`
 *
 * ```bash
 * pnpm mode-api-retry:fixtures
 * cargo test -p mode-api-retry
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  isModeModelRetryRecoveryProgressPayload,
  modeApiRetryFromModelNetworkStatusPayload,
  modeApiRetryFromStreamRecoveryPayload,
  normalizeModeApiRetryStatus,
} from "../packages/shared/src/mode-api-retry-status.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "mode-api-retry", "fixtures", "mode-api-retry.json");

type Outcome = { kind: "undefined" } | { kind: "null" } | { kind: "status"; status: unknown };

function encode(value: unknown): Outcome {
  if (value === undefined) return { kind: "undefined" };
  if (value === null) return { kind: "null" };
  return { kind: "status", status: JSON.parse(JSON.stringify(value)) as unknown };
}

const NORMALIZE_CASES: Array<{ name: string; hasValue: boolean; value?: unknown }> = [
  { name: "undefined", hasValue: false },
  { name: "null", hasValue: true, value: null },
  { name: "empty-object", hasValue: true, value: {} },
  { name: "array", hasValue: true, value: [] },
  { name: "scalar", hasValue: true, value: "x" },
  { name: "attempt", hasValue: true, value: { attempt: 2 } },
  { name: "next-attempt", hasValue: true, value: { nextAttempt: 5 } },
  { name: "zero-attempt", hasValue: true, value: { attempt: 0, nextAttempt: 5 } },
  { name: "max-attempts", hasValue: true, value: { attempt: 2, maxAttempts: 4 } },
  { name: "max-retries-below-attempt", hasValue: true, value: { attempt: 3, maxRetries: 1 } },
  { name: "delay-fallbacks", hasValue: true, value: { attempt: 1, retryDelayMs: 0, delayMs: 99 } },
  {
    name: "status-and-error-fallbacks",
    hasValue: true,
    value: { attempt: 1, errorStatus: 429, statusCode: 500, error: "e", message: "m" },
  },
  { name: "reason", hasValue: true, value: { attempt: 1, reason: "r" } },
  {
    name: "invalid-numbers",
    hasValue: true,
    value: {
      attempt: 1.5,
      nextAttempt: 4,
      maxRetries: -1,
      maxAttempts: 2.5,
      errorStatus: -1,
      statusCode: 503,
      message: "m",
    },
  },
];

const STREAM_CASES: Array<{ name: string; hasValue: boolean; value?: unknown }> = [
  { name: "undefined", hasValue: false },
  { name: "null", hasValue: true, value: null },
  { name: "empty", hasValue: true, value: {} },
  { name: "zero", hasValue: true, value: { retryNumber: 0 } },
  { name: "one", hasValue: true, value: { retryNumber: 1 } },
  { name: "with-max", hasValue: true, value: { retryNumber: 3, maxRetries: 2 } },
  {
    name: "message",
    hasValue: true,
    value: { retryNumber: 2, maxRetries: 5, message: "m" },
  },
  { name: "fraction", hasValue: true, value: { retryNumber: 1.5 } },
];

const NETWORK_CASES: Array<{ name: string; payload: Record<string, unknown> }> = [
  {
    name: "retry-scheduled",
    payload: { type: "model_retry_scheduled", attempt: 2, maxAttempts: 4, error: "x" },
  },
  { name: "retry-scheduled-next", payload: { type: "model_retry_scheduled", nextAttempt: 5 } },
  { name: "request-started-first", payload: { type: "model_request_started", attempt: 1 } },
  { name: "request-started-retry", payload: { type: "model_request_started", attempt: 3 } },
  {
    name: "request-started-stream",
    payload: {
      type: "model_request_started",
      attempt: 1,
      streamRecovery: { retryNumber: 2 },
    },
  },
  { name: "request-completed", payload: { type: "model_request_completed" } },
  { name: "failed-permanent", payload: { type: "model_request_failed", retryable: false } },
  { name: "failed-retryable", payload: { type: "model_request_failed", retryable: true } },
  { name: "unknown-type", payload: { type: "other" } },
];

const PROGRESS_CASES: Array<{ name: string; payload: Record<string, unknown> }> = [
  { name: "text-delta", payload: { kind: "text_delta", delta: "x" } },
  { name: "text-delta-empty", payload: { kind: "text_delta", delta: "" } },
  { name: "reasoning-delta", payload: { kind: "reasoning_delta", delta: " " } },
  { name: "tool-start", payload: { kind: "tool_input_start", toolCallId: "id" } },
  { name: "tool-start-empty-id", payload: { kind: "tool_input_start", toolCallId: "" } },
  { name: "tool-call", payload: { kind: "tool_call", toolCallId: "id" } },
  {
    name: "tool-input-delta",
    payload: { kind: "tool_input_delta", toolCallId: "id", delta: "d" },
  },
  {
    name: "tool-input-delta-empty",
    payload: { kind: "tool_input_delta", toolCallId: "id", delta: "" },
  },
  { name: "other-kind", payload: { kind: "other", toolCallId: "id" } },
  { name: "empty", payload: {} },
];

function main(): void {
  const fixture = {
    normalize: NORMALIZE_CASES.map((testCase) => ({
      name: testCase.name,
      hasValue: testCase.hasValue,
      value: testCase.value,
      expected: encode(
        testCase.hasValue
          ? normalizeModeApiRetryStatus(testCase.value)
          : normalizeModeApiRetryStatus(undefined),
      ),
    })),
    stream: STREAM_CASES.map((testCase) => ({
      name: testCase.name,
      hasValue: testCase.hasValue,
      value: testCase.value,
      expected: encode(
        testCase.hasValue
          ? modeApiRetryFromStreamRecoveryPayload(testCase.value)
          : modeApiRetryFromStreamRecoveryPayload(undefined),
      ),
    })),
    network: NETWORK_CASES.map((testCase) => ({
      name: testCase.name,
      payload: testCase.payload,
      expected: encode(modeApiRetryFromModelNetworkStatusPayload(testCase.payload)),
    })),
    progress: PROGRESS_CASES.map((testCase) => ({
      name: testCase.name,
      payload: testCase.payload,
      expected: isModeModelRetryRecoveryProgressPayload(testCase.payload),
    })),
  };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)}\n`);
}

main();
