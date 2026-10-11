import assert from "node:assert/strict";
import test from "node:test";

// 主控会话视图状态机（docs/specs/lan-remote-desktop.md 事件顺序 ③ 与降级语义）：
// 握手顺序（connected → hello → monitors → select-monitor → offer → answer/ice）、
// 首个结束原因生效、媒体失败收口、显示器切换只发信令不重建会话。
const { reduceRemoteDesktopSession, INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE } =
  await import("../src/remote-desktop/sessionReducer.js");
const { REMOTE_DESKTOP_PROTOCOL_VERSION } = await import("@mode/shared/remote-desktop");

const MONITORS = [
  {
    displayId: "display-2",
    label: "2",
    widthPx: 1920,
    heightPx: 1080,
    scaleFactor: 1,
    primary: false,
  },
  {
    displayId: "display-1",
    label: "1",
    widthPx: 2560,
    heightPx: 1440,
    scaleFactor: 1.25,
    primary: true,
  },
];

function reduce(state, event) {
  return reduceRemoteDesktopSession(state, event);
}

test("初始状态为连接中且无显示器", () => {
  assert.equal(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE.phase, "connecting");
  assert.deepEqual(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE.monitors, []);
  assert.equal(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE.selectedDisplayId, null);
});

test("信令通道 connected 后本侧发 hello{protocolVersion:1}", () => {
  const { state, effects } = reduce(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, {
    type: "controller",
    event: { kind: "state", state: "connected" },
  });
  assert.equal(state.phase, "connected");
  assert.deepEqual(effects, [
    {
      kind: "signal",
      message: { type: "hello", protocolVersion: REMOTE_DESKTOP_PROTOCOL_VERSION },
    },
  ]);
});

test("收到 monitors 自动选主屏并回 select-monitor", () => {
  const connected = reduce(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, {
    type: "controller",
    event: { kind: "state", state: "connected" },
  }).state;
  const { state, effects } = reduce(connected, {
    type: "controller",
    event: { kind: "signal", message: { type: "monitors", monitors: MONITORS } },
  });
  assert.equal(state.monitors.length, 2);
  assert.equal(state.selectedDisplayId, "display-1");
  assert.deepEqual(effects, [
    { kind: "signal", message: { type: "select-monitor", displayId: "display-1" } },
  ]);
});

test("重复 monitors 不重发 select-monitor（保持用户已选显示器）", () => {
  const selected = {
    ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE,
    phase: "connected",
    monitors: MONITORS,
    selectedDisplayId: "display-2",
  };
  const { state, effects } = reduce(selected, {
    type: "controller",
    event: { kind: "signal", message: { type: "monitors", monitors: MONITORS } },
  });
  assert.equal(state.selectedDisplayId, "display-2");
  assert.deepEqual(effects, []);
});

test("切换显示器只发 select-monitor：未知显示器与重复选择都不动作", () => {
  const base = {
    ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE,
    phase: "connected",
    monitors: MONITORS,
    selectedDisplayId: "display-1",
  };
  const switched = reduce(base, { type: "select-monitor", displayId: "display-2" });
  assert.equal(switched.state.selectedDisplayId, "display-2");
  assert.deepEqual(switched.effects, [
    { kind: "signal", message: { type: "select-monitor", displayId: "display-2" } },
  ]);
  assert.deepEqual(reduce(base, { type: "select-monitor", displayId: "display-9" }), {
    state: base,
    effects: [],
  });
  assert.deepEqual(reduce(base, { type: "select-monitor", displayId: "display-1" }), {
    state: base,
    effects: [],
  });
});

test("offer 进入协商并交由媒体面应答；ice 走 add-ice-candidate", () => {
  const offered = reduce(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, {
    type: "controller",
    event: { kind: "signal", message: { type: "offer", sdp: "v=0 offer" } },
  });
  assert.equal(offered.state.offerReceived, true);
  assert.equal(offered.state.mediaStatus, "negotiating");
  assert.deepEqual(offered.effects, [{ kind: "apply-offer", sdp: "v=0 offer" }]);

  const ice = reduce(offered.state, {
    type: "controller",
    event: {
      kind: "signal",
      message: { type: "ice", candidate: "candidate:1", sdpMid: "0", sdpMLineIndex: 0 },
    },
  });
  assert.deepEqual(ice.effects, [
    {
      kind: "add-ice-candidate",
      candidate: { candidate: "candidate:1", sdpMid: "0", sdpMLineIndex: 0 },
    },
  ]);
});

test("对端 error 记录稳定错误码，随后 closed 归因为错误", () => {
  const errored = reduce(
    { ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, phase: "connected" },
    {
      type: "controller",
      event: { kind: "signal", message: { type: "error", code: "consent_denied" } },
    },
  );
  assert.equal(errored.state.errorCode, "consent_denied");
  assert.equal(errored.state.closeKind, "error");

  const closed = reduce(errored.state, {
    type: "controller",
    event: { kind: "state", state: "closed", error: "consent_denied" },
  });
  assert.equal(closed.state.phase, "closed");
  assert.equal(closed.state.closeKind, "error");
  assert.equal(closed.state.errorCode, "consent_denied");
});

test("对端 bye 归因为远端结束", () => {
  const { state } = reduce(
    { ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, phase: "connected" },
    { type: "controller", event: { kind: "signal", message: { type: "bye", reason: "user-end" } } },
  );
  assert.equal(state.phase, "closed");
  assert.equal(state.closeKind, "remote");
  assert.equal(state.errorCode, null);
});

test("本端断开：先收口再发 end-session；迟到的 closed 不改写归因", () => {
  const ended = reduce(
    { ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, phase: "connected" },
    { type: "local-end" },
  );
  assert.equal(ended.state.phase, "closed");
  assert.equal(ended.state.closeKind, "user");
  assert.deepEqual(ended.effects, [{ kind: "end-session", reason: "user-end" }]);

  const late = reduce(ended.state, {
    type: "controller",
    event: { kind: "state", state: "closed", error: "busy" },
  });
  assert.equal(late.state.closeKind, "user");
  assert.equal(late.state.errorCode, null);
  assert.deepEqual(late.effects, []);
});

test("媒体失败终止会话并归因（不留半开状态）", () => {
  const live = reduce(
    { ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, phase: "connected" },
    { type: "media", status: "live" },
  );
  assert.equal(live.state.mediaStatus, "live");

  const failed = reduce(live.state, { type: "media", status: "failed" });
  assert.equal(failed.state.phase, "closed");
  assert.equal(failed.state.closeKind, "error");
  assert.equal(failed.state.mediaStatus, "failed");
  assert.deepEqual(failed.effects, [{ kind: "end-session", reason: "error" }]);
});

test("start-session 重试（connecting）重置上一轮归因", () => {
  const closed = {
    ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE,
    phase: "closed",
    closeKind: "error",
    errorCode: "busy",
  };
  const retrying = reduce(closed, {
    type: "controller",
    event: { kind: "state", state: "connecting" },
  });
  assert.equal(retrying.state.phase, "connecting");
  assert.equal(retrying.state.closeKind, null);
  assert.equal(retrying.state.errorCode, null);
  assert.equal(retrying.state.offerReceived, false);
});
