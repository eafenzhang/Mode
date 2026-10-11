/**
 * 被控远程桌面会话状态机夹具（spec: docs/specs/lan-remote-desktop.md 状态所有者「被控会话状态机」、
 * 产品规则 2、事件顺序 ②⑥）。纯函数断言，跨平台可跑。
 * 失效模式（未实现时）：已授权设备仍弹同意窗 / 忙时允许第二个请求 / 拒绝路径不进 closed /
 * close 不自幂等 / 收口后回不到 idle → 断言失败。
 */
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// services 在模块加载时读 MODE_DATA_BASE_DIR（会话模块自身不依赖，但保持与本目录夹具一致的启动纪律）。
process.env.MODE_DATA_BASE_DIR = await mkdtemp(join(tmpdir(), "mode-rd-session-"));
const {
  INITIAL_REMOTE_DESKTOP_SESSION_STATE,
  isRemoteDesktopSessionBusy,
  reduceRemoteDesktopSession,
} = await import("../../src/main/remoteDesktop/sessionState.js");

const kinds = (actions) => actions.map((action) => action.kind);

// ── 未授权设备：request → requesting（弹同意窗），不直接进 active ──
const requested = reduceRemoteDesktopSession(INITIAL_REMOTE_DESKTOP_SESSION_STATE, {
  type: "request",
  clientId: "client-a",
  label: "Office-PC",
  authorized: false,
});
assert.equal(requested.state.phase, "requesting");
assert.equal(requested.state.clientId, "client-a");
assert.equal(requested.state.label, "Office-PC");
assert.equal(requested.rejected, undefined);
assert.deepEqual(kinds(requested.actions), ["open-consent"]);
assert.equal(isRemoteDesktopSessionBusy(requested.state), true, "requesting 属于忙");

// ── 同意窗就绪 → waitingConsent ──
const waiting = reduceRemoteDesktopSession(requested.state, { type: "consent-shown" });
assert.equal(waiting.state.phase, "waitingConsent");
assert.deepEqual(waiting.actions, []);

// ── 允许（含记住）→ active：先关同意窗、回帖、起指示、起采集 ──
const allowed = reduceRemoteDesktopSession(waiting.state, {
  type: "consent-decided",
  outcome: "allow",
  remember: true,
});
assert.equal(allowed.state.phase, "active");
assert.equal(allowed.state.clientId, "client-a");
assert.deepEqual(kinds(allowed.actions), [
  "close-consent",
  "answer-consent",
  "start-indicator",
  "start-capture",
]);
const allowAnswer = allowed.actions.find((action) => action.kind === "answer-consent");
assert.deepEqual(allowAnswer, { kind: "answer-consent", outcome: "allow", remember: true });

// ── 已记住设备（authorized）：跳过同意窗直接 active，且不回写授权 ──
const authorized = reduceRemoteDesktopSession(INITIAL_REMOTE_DESKTOP_SESSION_STATE, {
  type: "request",
  clientId: "client-b",
  label: "Laptop",
  authorized: true,
});
assert.equal(authorized.state.phase, "active");
assert.deepEqual(kinds(authorized.actions), [
  "answer-consent",
  "start-indicator",
  "start-capture",
]);
assert.deepEqual(authorized.actions.find((action) => action.kind === "answer-consent"), {
  kind: "answer-consent",
  outcome: "allow",
  remember: false,
});

// ── 单会话互斥：非 idle 相位的第二个 request → busy，且状态不迁移 ──
for (const state of [requested.state, waiting.state, allowed.state]) {
  const busy = reduceRemoteDesktopSession(state, {
    type: "request",
    clientId: "client-c",
    label: "Intruder",
    authorized: true,
  });
  assert.equal(busy.rejected, "busy", `${state.phase} 相位必须拒绝第二请求`);
  assert.equal(busy.state, state, "被拒请求不得改变状态");
  assert.deepEqual(busy.actions, []);
}
const busyAfterClose = reduceRemoteDesktopSession(
  { phase: "closed", clientId: "client-a", label: "Office-PC", closeReason: "peer-end" },
  { type: "request", clientId: "client-c", label: "Intruder", authorized: true },
);
assert.equal(busyAfterClose.rejected, "busy", "收口未完成（closed）期间仍不得放行新会话");

// ── 拒绝 / 超时：进 closed(consent-denied)，回帖带出 outcome 供 host 映射稳定错误码 ──
const denied = reduceRemoteDesktopSession(waiting.state, {
  type: "consent-decided",
  outcome: "deny",
  remember: false,
});
assert.equal(denied.state.phase, "closed");
assert.equal(denied.state.closeReason, "consent-denied");
assert.deepEqual(kinds(denied.actions), ["close-consent", "answer-consent"]);
assert.deepEqual(denied.actions.find((action) => action.kind === "answer-consent"), {
  kind: "answer-consent",
  outcome: "deny",
  remember: false,
});

const timedOut = reduceRemoteDesktopSession(requested.state, {
  type: "consent-decided",
  outcome: "timeout",
  remember: true,
});
assert.equal(timedOut.state.closeReason, "consent-denied");
assert.deepEqual(timedOut.actions.find((action) => action.kind === "answer-consent"), {
  kind: "answer-consent",
  outcome: "timeout",
  remember: false,
  // timeout 时不得写授权（remember 被忽略）。
});

// ── 迟到消息：closed 之后到达的 consent-decided 不改变状态 ──
const late = reduceRemoteDesktopSession(denied.state, {
  type: "consent-decided",
  outcome: "allow",
  remember: true,
});
assert.equal(late.state, denied.state);
assert.deepEqual(late.actions, []);

// ── 会话中收口：active + close(local-end) → closed，先停采集再撤指示 ──
const closed = reduceRemoteDesktopSession(allowed.state, { type: "close", reason: "local-end" });
assert.equal(closed.state.phase, "closed");
assert.equal(closed.state.closeReason, "local-end");
assert.deepEqual(kinds(closed.actions), ["stop-capture", "stop-indicator"]);

// ── close 幂等：首个原因生效，重复 close 无动作 ──
const closedAgain = reduceRemoteDesktopSession(closed.state, { type: "close", reason: "peer-end" });
assert.equal(closedAgain.state, closed.state, "重复 close 不得改写原因");
assert.deepEqual(closedAgain.actions, []);

// ── 同意等待期被收口：只关同意窗，不触采集/指示 ──
const closedWhileWaiting = reduceRemoteDesktopSession(waiting.state, {
  type: "close",
  reason: "app-quit",
});
assert.deepEqual(kinds(closedWhileWaiting.actions), ["close-consent"]);

// ── cleanup-done → 回 idle，允许下一个会话 ──
const idleAgain = reduceRemoteDesktopSession(closed.state, { type: "cleanup-done" });
assert.deepEqual(idleAgain.state, INITIAL_REMOTE_DESKTOP_SESSION_STATE);
assert.deepEqual(idleAgain.actions, []);
const nextRequest = reduceRemoteDesktopSession(idleAgain.state, {
  type: "request",
  clientId: "client-a",
  label: "Office-PC",
  authorized: true,
});
assert.equal(nextRequest.state.phase, "active", "收口后新请求必须被受理");

console.log("REMOTE_DESKTOP_SESSION_OK");
