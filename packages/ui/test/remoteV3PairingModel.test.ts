import assert from "node:assert/strict";
import test from "node:test";

import {
  INITIAL_REMOTE_V3_PAIRING_UI,
  reduceRemoteV3Pairing,
  type RemoteV3PairingUiState,
} from "../src/account/pairingModel.js";

test("Host：ticket → 对方认领 → 输入验证码 → 完成", () => {
  let state: RemoteV3PairingUiState = reduceRemoteV3Pairing(INITIAL_REMOTE_V3_PAIRING_UI, {
    type: "host-started",
    pairingId: "p-1",
    ticket: "ticket-1",
    expiresAt: 100,
  });
  assert.deepEqual(state, {
    kind: "host",
    pairingId: "p-1",
    ticket: "ticket-1",
    expiresAt: 100,
    claimed: false,
    code: "",
  });

  state = reduceRemoteV3Pairing(state, { type: "host-code", code: "12345678" });
  assert.equal(state.kind === "host" ? state.code : null, "", "未认领前不接受验证码输入");

  state = reduceRemoteV3Pairing(state, { type: "host-done" });
  assert.equal(state.kind, "host", "未认领前不能完成");

  state = reduceRemoteV3Pairing(state, { type: "host-claimed" });
  state = reduceRemoteV3Pairing(state, { type: "host-code", code: "12345678" });
  assert.equal(state.kind === "host" ? state.code : null, "12345678");

  state = reduceRemoteV3Pairing(state, { type: "host-done" });
  assert.equal(state.kind, "host-done");
});

test("Client：claim → 等待确认 → 完成", () => {
  let state = reduceRemoteV3Pairing(INITIAL_REMOTE_V3_PAIRING_UI, {
    type: "client-claimed",
    pairingId: "p-2",
    verificationCode: "87654321",
    hostName: "Desk",
    expiresAt: 200,
  });
  assert.deepEqual(state, {
    kind: "client",
    pairingId: "p-2",
    verificationCode: "87654321",
    hostName: "Desk",
    expiresAt: 200,
  });
  state = reduceRemoteV3Pairing(state, { type: "client-done" });
  assert.equal(state.kind, "client-done");
  assert.equal(reduceRemoteV3Pairing(state, { type: "host-claimed" }).kind, "client-done");
});

test("失败与重置回到可重新开始的状态", () => {
  const failed = reduceRemoteV3Pairing(
    { kind: "host-done" },
    { type: "failed", message: "pairing_expired" },
  );
  assert.deepEqual(failed, { kind: "error", message: "pairing_expired" });
  assert.deepEqual(reduceRemoteV3Pairing(failed, { type: "reset" }), { kind: "idle" });
});
