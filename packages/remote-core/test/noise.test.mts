import assert from "node:assert/strict";
import test from "node:test";

import { generateIdentity } from "../src/adapters/identity.js";
import { NoiseHandshake, NoiseTransport } from "../src/adapters/noise.js";
import { RemoteError } from "../src/domain/errors.js";
import { buildPrologue } from "../src/domain/prologue.js";

function pair(overrides?: { prologueHost?: string; expectedClient?: string }) {
  const host = generateIdentity();
  const client = generateIdentity();
  const prologue = buildPrologue({
    connectionId: "conn-1",
    hostDeviceId: "host-1",
    clientDeviceId: "client-1",
  });
  const initiator = new NoiseHandshake({
    initiator: true,
    localIdentity: client,
    responderStaticKey: host.identityKey,
    prologue,
  });
  const responder = new NoiseHandshake({
    initiator: false,
    localIdentity: host,
    responderStaticKey: host.identityKey,
    expectedPeerStaticKey: overrides?.expectedClient ?? client.identityKey,
    prologue: overrides?.prologueHost
      ? buildPrologue({
          connectionId: overrides.prologueHost,
          hostDeviceId: "host-1",
          clientDeviceId: "client-1",
        })
      : prologue,
  });
  return { host, client, initiator, responder };
}

test("Noise IK 完成两消息握手并派生对称 transport key", () => {
  const { initiator, responder } = pair();
  const m1 = initiator.writeMessage();
  assert.equal(m1.length, 80);
  responder.readMessage(m1);
  const m2 = responder.writeMessage();
  assert.equal(m2.length, 32);
  initiator.readMessage(m2);
  assert.equal(initiator.isComplete, true);
  assert.equal(responder.isComplete, true);

  const initPair = initiator.split();
  const respPair = responder.split();
  assert.deepEqual(initPair.sendKey, respPair.receiveKey);
  assert.deepEqual(initPair.receiveKey, respPair.sendKey);

  const a = new NoiseTransport(initPair);
  const b = new NoiseTransport(respPair);
  const ct = a.send(Buffer.from("hello host"));
  assert.deepEqual(b.receive(ct), Buffer.from("hello host"));
  const back = b.send(Buffer.from("hello client"));
  assert.deepEqual(a.receive(back), Buffer.from("hello client"));
});

test("Noise 拒绝篡改、重放与身份替换", () => {
  const { initiator, responder } = pair();
  responder.readMessage(initiator.writeMessage());
  responder.writeMessage();
  // 独立完成一次合法握手用于传输层测试。
  const again = pair();
  again.responder.readMessage(again.initiator.writeMessage());
  again.initiator.readMessage(again.responder.writeMessage());
  const a = new NoiseTransport(again.initiator.split());
  const b = new NoiseTransport(again.responder.split());
  const ct = a.send(Buffer.from("payload"));
  const tampered = Buffer.from(ct);
  tampered[0] ^= 0xff;
  assert.throws(() => b.receive(tampered), RemoteError);
  assert.throws(() => b.receive(ct), RemoteError);
});

test("Noise 拒绝错误的 prologue 与 peer static key", () => {
  const prologueMismatch = pair({ prologueHost: "conn-2" });
  assert.throws(
    () => prologueMismatch.responder.readMessage(prologueMismatch.initiator.writeMessage()),
    (error: unknown) =>
      error instanceof RemoteError && error.code === "noise_auth_failed",
  );

  const identityMismatch = pair({ expectedClient: generateIdentity().identityKey });
  assert.throws(
    () => identityMismatch.responder.readMessage(identityMismatch.initiator.writeMessage()),
    (error: unknown) =>
      error instanceof RemoteError && error.code === "identity_mismatch",
  );
});
