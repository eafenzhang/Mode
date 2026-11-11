import assert from "node:assert/strict";
import test from "node:test";

import { generateIdentity } from "../src/adapters/identity.js";
import type { BusinessFrame } from "../src/domain/businessFrames.js";
import { RemoteError } from "../src/domain/errors.js";
import { SecureSession } from "../src/app/secureSession.js";

interface Endpoint {
  session: SecureSession;
  data: Uint8Array[];
  frames: BusinessFrame[];
  errors: RemoteError[];
  ready: boolean;
}

function createPair(): { host: Endpoint; client: Endpoint; drive: () => void } {
  const hostIdentity = generateIdentity();
  const clientIdentity = generateIdentity();
  const context = {
    connectionId: "conn-1",
    hostDeviceId: "host-1",
    clientDeviceId: "client-1",
  };
  const hostData: Uint8Array[] = [];
  const clientData: Uint8Array[] = [];
  const host: Endpoint = {
    session: undefined as never,
    data: hostData,
    frames: [],
    errors: [],
    ready: false,
  };
  const client: Endpoint = {
    session: undefined as never,
    data: clientData,
    frames: [],
    errors: [],
    ready: false,
  };
  const pendingToHost: Array<[1 | 2, Uint8Array]> = [];
  const pendingToClient: Array<[1 | 2, Uint8Array]> = [];

  host.session = new SecureSession({
    role: "host",
    localIdentity: hostIdentity,
    peerIdentityKey: clientIdentity.identityKey,
    context,
    sendHandshake: (step, data) => pendingToClient.push([step, data]),
    onFrame: (frame) => host.frames.push(frame),
    onReady: () => {
      host.ready = true;
    },
    onError: (error) => host.errors.push(error),
    onClose: () => undefined,
  });
  client.session = new SecureSession({
    role: "client",
    localIdentity: clientIdentity,
    peerIdentityKey: hostIdentity.identityKey,
    context,
    sendHandshake: (step, data) => pendingToHost.push([step, data]),
    onFrame: (frame) => client.frames.push(frame),
    onReady: () => {
      client.ready = true;
    },
    onError: (error) => client.errors.push(error),
    onClose: () => undefined,
  });
  host.session.attachData((bytes) => clientData.push(bytes));
  client.session.attachData((bytes) => hostData.push(bytes));

  const drive = (): void => {
    client.session.start();
    while (pendingToHost.length > 0 || pendingToClient.length > 0) {
      const toHost = pendingToHost.shift();
      if (toHost) host.session.handleHandshake(toHost[0], toHost[1]);
      const toClient = pendingToClient.shift();
      if (toClient) client.session.handleHandshake(toClient[0], toClient[1]);
    }
  };
  return { host, client, drive };
}

test("SecureSession 完成握手并通过数据面双向收发", () => {
  const { host, client, drive } = createPair();
  drive();
  assert.equal(host.ready, true);
  assert.equal(client.ready, true);

  client.session.send({ v: 1, type: "ping", nonce: "n1" });
  assert.equal(host.data.length, 1);
  host.session.handleData(host.data.shift()!);
  assert.deepEqual(host.frames[0], { v: 1, type: "ping", nonce: "n1" });

  host.session.send({ v: 1, type: "pong", nonce: "n1" });
  assert.equal(client.data.length, 1);
  client.session.handleData(client.data.shift()!);
  assert.deepEqual(client.frames[0], { v: 1, type: "pong", nonce: "n1" });
});

test("SecureSession 自动分片大于 48 KiB 的业务消息", () => {
  const { host, client, drive } = createPair();
  drive();
  const result = { body: "x".repeat(150_000) };
  client.session.send({ v: 1, type: "rpc.response", id: "r1", result });
  assert.ok(host.data.length > 1);
  while (host.data.length > 0) {
    host.session.handleData(host.data.shift()!);
  }
  assert.deepEqual(host.frames[0], { v: 1, type: "rpc.response", id: "r1", result });
});

test("SecureSession 遇到篡改密文时 fail closed", () => {
  const { host, client, drive } = createPair();
  drive();
  client.session.send({ v: 1, type: "ping" });
  const bytes = host.data.shift()!;
  bytes[0] ^= 0xff;
  host.session.handleData(bytes);
  assert.equal(host.session.currentState, "closed");
  assert.equal(host.errors[0]?.code, "noise_auth_failed");
});
