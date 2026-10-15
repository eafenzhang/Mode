import assert from "node:assert/strict";
import test from "node:test";

// 主控会话视图状态机（docs/specs/lan-remote-desktop-native-media.md 事件顺序 ② 与降级语义）：
// 握手顺序（connected → hello → monitors → select-monitor → media-open → open-media）、
// 首个结束原因生效、媒体失败收口、显示器切换只发信令不重建会话。
const { reduceRemoteDesktopSession, INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE } =
  await import("../src/remote-desktop/sessionReducer.js");
const { REMOTE_DESKTOP_PROTOCOL_VERSION, remoteDesktopSignalMessageSchema } =
  await import("@mode/shared/remote-desktop");

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
  assert.equal(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE.mediaOpenReceived, false);
  assert.equal(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE.awaitingFirstFrame, false);
  assert.equal(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE.frozen, false);
});

test("首帧看门狗：零帧切可见提示、首帧撤除、会话重建/媒体面重建清零", () => {
  // spec「媒体面可观测性与首帧约束」（2026-10-14）：mediaStatus 已 live（媒体面对象已建，
  // 早于任何一帧）并不代表有画面——「连上但一帧不出」必须由单独的可见位表达。
  const live = reduce(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, {
    type: "media",
    status: "live",
  }).state;
  assert.equal(live.awaitingFirstFrame, false, "live 本身不置缺帧位");

  const stalled = reduce(live, { type: "first-frame", seen: false });
  assert.equal(stalled.state.awaitingFirstFrame, true, "看门狗标出零帧");
  assert.deepEqual(stalled.effects, [], "只切可见位，不发信令、不改媒体态");
  assert.equal(stalled.state.mediaStatus, "live", "媒体态保持 live（传输仍在）");

  // 重复触发（重连后再触发）幂等：状态内容不变、不改媒体态。
  const replayed = reduce(stalled.state, { type: "first-frame", seen: false }).state;
  assert.equal(replayed.awaitingFirstFrame, true, "重复触发仍为缺帧态");
  assert.equal(replayed.mediaStatus, "live", "重复触发不改媒体态");

  const resumed = reduce(stalled.state, { type: "first-frame", seen: true });
  assert.equal(resumed.state.awaitingFirstFrame, false, "首个已解码帧撤除提示");

  // 会话重建（connecting 迁移）清零：新会话必须由首帧或看门狗重新定态。
  assert.equal(
    reduce(stalled.state, { type: "controller", event: { kind: "state", state: "connecting" } })
      .state.awaitingFirstFrame,
    false,
    "connecting 迁移清零",
  );

  // 媒体面重建（新 media-open）清零：新 socket 从零计时。
  assert.equal(
    reduce(stalled.state, {
      type: "controller",
      event: {
        kind: "signal",
        message: { type: "media-open", key: "ab".repeat(32), salt: "cd".repeat(8) },
      },
    }).state.awaitingFirstFrame,
    false,
    "media-open 迁移清零",
  );
});

test("信令通道 connected 后本侧发 hello{protocolVersion:2}", () => {
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
    // 同会话内已发过 select-monitor（media-open 已到）：重复 monitors 只刷新列表不重发。
    selectSentForSession: true,
  };
  const { state, effects } = reduce(selected, {
    type: "controller",
    event: { kind: "signal", message: { type: "monitors", monitors: MONITORS } },
  });
  assert.equal(state.selectedDisplayId, "display-2");
  assert.deepEqual(effects, []);
});

test("会话重建（connecting 后）重复显示器列表重发 select-monitor（media-open 逐会话）", () => {
  // 2026-10-13 实机缺陷：改档/断线全重建走 bye → 重 POST，新会话的 media-open 仍只随
  // select-monitor 应答下发——但旧状态保留 selectedDisplayId，若不重发，主控永远拿不到
  // 新会话 key/salt，画面冻结在旧帧（spec 事件顺序 ③ 会话级握手）。
  const midSession = {
    ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE,
    phase: "connected" as const,
    monitors: MONITORS,
    selectedDisplayId: "display-2",
    selectSentForSession: true,
    mediaOpenReceived: true,
  };
  // 重建：closed → connecting（重置会话级欠账）→ connected → monitors（同列表）。
  const closed = reduce(midSession, {
    type: "controller",
    event: { kind: "state", state: "closed" },
  }).state;
  const reconnecting = reduce(closed, {
    type: "controller",
    event: { kind: "state", state: "connecting" },
  }).state;
  assert.equal(reconnecting.selectSentForSession, false, "connecting 必须清会话级欠账");
  assert.equal(reconnecting.mediaOpenReceived, false, "media-open 属新会话，不得继承");
  const reconnected = reduce(reconnecting, {
    type: "controller",
    event: { kind: "state", state: "connected" },
  }).state;
  const { state, effects } = reduce(reconnected, {
    type: "controller",
    event: { kind: "signal", message: { type: "monitors", monitors: MONITORS } },
  });
  assert.equal(state.selectedDisplayId, "display-2", "记住用户选择的显示器");
  assert.equal(state.selectSentForSession, true);
  assert.deepEqual(
    effects,
    [{ kind: "signal", message: { type: "select-monitor", displayId: "display-2" } }],
    "新会话必须对记住的显示器重发 select-monitor（否则拿不到本会话 media-open）",
  );
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

test("media-open：进入 negotiating 并产出 open-media effect", () => {
  const state = { ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, phase: "connected" as const };
  const t = reduce(state, {
    type: "controller",
    event: {
      kind: "signal",
      message: { type: "media-open", key: "a".repeat(64), salt: "b".repeat(16) },
    },
  });
  assert.equal(t.state.mediaOpenReceived, true);
  assert.equal(t.state.mediaStatus, "negotiating");
  assert.deepEqual(t.effects, [{ kind: "open-media", key: "a".repeat(64), salt: "b".repeat(16) }]);
});

test("WebRTC 残留信令已随 P4 从协议面移除：offer/answer/ice 在 renderer 边界即被拒", () => {
  // 状态机已不再有 offer/ice 分支（Task 14 删除），reducer 不再需要「记录性忽略」；
  // 契约侧必须同步拒绝这三条老消息，否则 main 转发的帧会在边界内漏进 reducer。
  for (const message of [
    { type: "offer", sdp: "v=0 offer" },
    { type: "answer", sdp: "v=0 answer" },
    { type: "ice", candidate: "candidate:1", sdpMid: "0", sdpMLineIndex: 0 },
  ]) {
    assert.equal(remoteDesktopSignalMessageSchema.safeParse(message).success, false);
  }
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
  assert.equal(retrying.state.offerReceived === undefined, true, "旧字段已随 Task 10 改名");
  assert.equal(retrying.state.mediaOpenReceived, false);
});

test("freeze/unfreeze 信令切换 frozen；connecting 重连重置", () => {
  // 被控端锁屏/失去访问权限时发 freeze（reason 已在信令层归一，UI 只需布尔）；
  // 恢复后发 unfreeze。重连（connecting）必须清掉 frozen：新会话不能继承上一轮的冻结标记。
  const live = { ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, phase: "connected" };

  const frozen = reduce(live, {
    type: "controller",
    event: { kind: "signal", message: { type: "freeze", reason: "locked" } },
  });
  assert.equal(frozen.state.frozen, true);
  assert.deepEqual(frozen.effects, [], "冻结只改视图状态，不产出副作用");

  const thawed = reduce(frozen.state, {
    type: "controller",
    event: { kind: "signal", message: { type: "unfreeze" } },
  });
  assert.equal(thawed.state.frozen, false);

  // access-lost 与 locked 同分支：reason 不进 state（Task 6 已归一为两值，当前文案不区分）。
  const accessLost = reduce(live, {
    type: "controller",
    event: { kind: "signal", message: { type: "freeze", reason: "access-lost" } },
  });
  assert.equal(accessLost.state.frozen, true);

  const retrying = reduce(accessLost.state, {
    type: "controller",
    event: { kind: "state", state: "connecting" },
  });
  assert.equal(retrying.state.frozen, false);
});

// 断线重连（P2-1）：媒体状态 reconnecting 透传且会话面保持挂载；
// 重连控制面信号（request-media-ticket / media-ticket）防御性忽略。
test("重连状态：reconnecting 只改 mediaStatus，不把会话收口", () => {
  const live = {
    ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE,
    phase: "connected",
    mediaStatus: "live",
  };
  const reconnecting = reduceRemoteDesktopSession(live, {
    type: "media",
    status: "reconnecting",
  });
  assert.equal(reconnecting.state.mediaStatus, "reconnecting");
  assert.equal(reconnecting.state.phase, "connected", "重连中会话面保持挂载（不 closed）");
  assert.deepEqual(reconnecting.effects, [], "重连状态只改视图，不产出副作用");

  // 重连成功（媒体面恢复）→ live；失败仍走原 media-failed 收口语义。
  const recovered = reduceRemoteDesktopSession(reconnecting.state, {
    type: "media",
    status: "live",
  });
  assert.equal(recovered.state.mediaStatus, "live");
  const failed = reduceRemoteDesktopSession(reconnecting.state, {
    type: "media",
    status: "failed",
  });
  assert.equal(failed.state.phase, "closed", "退避耗尽后仍按媒体失败收口");
});

test("重连控制面信号到达 reducer 不改状态（正常由会话胶水拦截）", () => {
  const live = {
    ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE,
    phase: "connected",
    mediaStatus: "live",
  };
  const ticketReply = reduceRemoteDesktopSession(live, {
    type: "controller",
    event: { kind: "signal", message: { type: "media-ticket", mediaTicket: "t-1" } },
  });
  assert.deepEqual(ticketReply.state, live, "media-ticket 不改变视图状态");
  assert.deepEqual(ticketReply.effects, []);
  const ticketRequest = reduceRemoteDesktopSession(live, {
    type: "controller",
    event: { kind: "signal", message: { type: "request-media-ticket" } },
  });
  assert.deepEqual(ticketRequest.state, live, "request-media-ticket 不改变视图状态");
  assert.deepEqual(ticketRequest.effects, []);
});
