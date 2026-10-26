/**
 * `canRetryDatabaseStartup` 的判据用例（2026-10-26）。
 *
 * 背景：Tauri 线的「重试」不是在同一连接上重发，而是让壳**重生 Host 换代**
 * （spec `tauri-renderer-host.md` §22.4）。所以判据要按「重启进程能不能变好」来分：
 * 进程死亡类必须给重试；数据损坏类与已部分装配服务不给。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  canRetryDatabaseStartup,
  type DatabaseStartupErrorCode,
  type DatabaseStartupPhase,
} from "../src/database-startup.js";

function failed(errorCode: DatabaseStartupErrorCode, failedPhase: DatabaseStartupPhase = "starting") {
  return { phase: "failed" as const, failedPhase, errorCode };
}

test("Host 死亡类失败必须给重试（retry = 重生换代）", () => {
  // 壳的 reaper 发布的就是这两条：进程退出 / 就绪期限超时。
  assert.equal(canRetryDatabaseStartup(failed("transport_closed")), true);
  assert.equal(canRetryDatabaseStartup(failed("startup_status_timeout")), true);
  assert.equal(canRetryDatabaseStartup(failed("open_failed", "preparing_host_storage")), true);
  assert.equal(canRetryDatabaseStartup(failed("sql_failed", "migrating")), true);
});

test("数据损坏或运行环境不支持时不重试（重启同级进程不会变好）", () => {
  for (const code of [
    "corrupt",
    "checksum_mismatch",
    "unsupported_runtime",
  ] as const) {
    assert.equal(canRetryDatabaseStartup(failed(code)), false, `${code} 不该给重试`);
  }
});

test("已部分装配服务（starting_services）不重试，避免重复启动副作用", () => {
  assert.equal(canRetryDatabaseStartup(failed("sql_failed", "starting_services")), false);
});

test("非 failed 阶段一律不重试", () => {
  assert.equal(
    canRetryDatabaseStartup({ phase: "starting", failedPhase: undefined, errorCode: undefined }),
    false,
  );
  assert.equal(
    canRetryDatabaseStartup({ phase: "ready", failedPhase: undefined, errorCode: undefined }),
    false,
  );
});

test("缺 errorCode 时按 sql_failed 处理（可重试）", () => {
  assert.equal(
    canRetryDatabaseStartup({ phase: "failed", failedPhase: "starting", errorCode: undefined }),
    true,
  );
});
