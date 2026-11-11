import assert from "node:assert/strict";
import test from "node:test";

import {
  NoiseHandshake,
  NoiseTransport,
  buildPrologue,
  generateIdentity,
} from "../src/index.js";
import {
  WebNoiseHandshake,
  WebNoiseTransport,
  generateWebIdentity,
} from "../src/web/index.js";

const context = {
  connectionId: "conn-web-1",
  hostDeviceId: "host-web-1",
  clientDeviceId: "client-web-1",
};
const prologue = buildPrologue(context);

function text(bytes: Uint8Array): string {
  return new TextDecoder().decode(bytes);
}

test("Node Initiator ↔ Web Responder 完成 Noise IK 并双向收发", () => {
  const nodeClient = generateIdentity();
  const webHost = generateWebIdentity();
  const node = new NoiseHandshake({
    initiator: true,
    localIdentity: nodeClient,
    responderStaticKey: webHost.identityKey,
    prologue,
  });
  const web = new WebNoiseHandshake({
    initiator: false,
    localIdentity: webHost,
    responderStaticKey: webHost.identityKey,
    expectedPeerStaticKey: nodeClient.identityKey,
    prologue,
  });

  web.readMessage(node.writeMessage());
  node.readMessage(web.writeMessage());
  assert.equal(node.isComplete, true);
  assert.equal(web.isComplete, true);

  const nodeTransport = new NoiseTransport(node.split());
  const webTransport = new WebNoiseTransport(web.split());
  assert.equal(text(webTransport.receive(nodeTransport.send(new TextEncoder().encode("hello-web")))), "hello-web");
  assert.equal(text(nodeTransport.receive(webTransport.send(new TextEncoder().encode("hello-node")))), "hello-node");
});

test("Web Initiator ↔ Node Responder 完成 Noise IK 并拒绝错误 static key", () => {
  const webClient = generateWebIdentity();
  const nodeHost = generateIdentity();
  const web = new WebNoiseHandshake({
    initiator: true,
    localIdentity: webClient,
    responderStaticKey: nodeHost.identityKey,
    prologue,
  });
  const node = new NoiseHandshake({
    initiator: false,
    localIdentity: nodeHost,
    responderStaticKey: nodeHost.identityKey,
    expectedPeerStaticKey: webClient.identityKey,
    prologue,
  });

  node.readMessage(web.writeMessage());
  web.readMessage(node.writeMessage());
  const webTransport = new WebNoiseTransport(web.split());
  const nodeTransport = new NoiseTransport(node.split());
  assert.equal(text(nodeTransport.receive(webTransport.send(new TextEncoder().encode("from-web")))), "from-web");

  const wrongClient = generateWebIdentity();
  const attacker = new WebNoiseHandshake({
    initiator: true,
    localIdentity: wrongClient,
    responderStaticKey: nodeHost.identityKey,
    prologue,
  });
  const victim = new NoiseHandshake({
    initiator: false,
    localIdentity: nodeHost,
    responderStaticKey: nodeHost.identityKey,
    expectedPeerStaticKey: webClient.identityKey,
    prologue,
  });
  assert.throws(() => victim.readMessage(attacker.writeMessage()), /static key/);
});
