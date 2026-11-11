import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import test from "node:test";
import { WebSocket } from "ws";

import {
  SecureSession,
  decodeBase64Url,
  decodeControlFrame,
  encodeBase64Url,
  encodeControlFrame,
  generateIdentity,
  type BusinessFrame,
  type ControlFrame,
  type DeviceDescriptor,
} from "@mode/remote-core";

import { startRemoteServer, type RemoteServerHandle } from "../src/index.js";

const HOST_ID = "019a0000-0000-7000-8000-000000000011";
const CLIENT_ID = "019a0000-0000-7000-8000-000000000012";

function frame<T extends ControlFrame["type"]>(
  type: T,
  payload: Extract<ControlFrame, { type: T }>["payload"],
): ControlFrame {
  return { v: 1, id: randomUUID(), timestamp: Date.now(), type, payload } as ControlFrame;
}

class Wire {
  readonly ws: WebSocket;
  private readonly frames: ControlFrame[] = [];
  private readonly waiters: Array<{
    predicate: (frame: ControlFrame) => boolean;
    resolve: (frame: ControlFrame) => void;
    reject: (error: Error) => void;
    timer: NodeJS.Timeout;
  }> = [];

  constructor(url: string) {
    this.ws = new WebSocket(url);
    this.ws.on("message", (data, isBinary) => {
      if (isBinary) return;
      const bytes = Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer);
      const decoded = decodeControlFrame(bytes);
      const index = this.waiters.findIndex((waiter) => waiter.predicate(decoded));
      if (index >= 0) {
        const [waiter] = this.waiters.splice(index, 1);
        clearTimeout(waiter!.timer);
        waiter!.resolve(decoded);
      } else {
        this.frames.push(decoded);
      }
    });
  }

  async open(): Promise<void> {
    if (this.ws.readyState === WebSocket.OPEN) return;
    await new Promise<void>((resolve, reject) => {
      this.ws.once("open", () => resolve());
      this.ws.once("error", reject);
    });
  }

  send(value: ControlFrame): void {
    this.ws.send(Buffer.from(encodeControlFrame(value)).toString("utf8"));
  }

  next(
    predicate: (frame: ControlFrame) => boolean,
    timeoutMs = 5000,
  ): Promise<ControlFrame> {
    const index = this.frames.findIndex(predicate);
    if (index >= 0) {
      return Promise.resolve(this.frames.splice(index, 1)[0]!);
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        const index = this.waiters.findIndex((waiter) => waiter.timer === timer);
        if (index >= 0) this.waiters.splice(index, 1);
        reject(new Error("等待 control frame 超时"));
      }, timeoutMs);
      this.waiters.push({ predicate, resolve, reject, timer });
    });
  }

  close(): void {
    for (const waiter of this.waiters.splice(0)) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error("wire closed"));
    }
    this.ws.close();
  }
}

async function loginAndRegister(
  server: RemoteServerHandle,
  descriptors: DeviceDescriptor[],
): Promise<Map<string, { accessToken: string }>> {
  const login = await fetch(`${server.url}/api/v1/auth/github`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ accessToken: "github-e2e" }),
  });
  const loginText = await login.text();
  assert.equal(login.status, 200, loginText);
  const { sessionToken } = JSON.parse(loginText) as { sessionToken: string };
  const result = new Map<string, { accessToken: string }>();
  for (const descriptor of descriptors) {
    const response = await fetch(`${server.url}/api/v1/devices`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${sessionToken}`,
      },
      body: JSON.stringify({ device: descriptor }),
    });
    const text = await response.text();
    assert.equal(response.status, 201, text);
    const body = JSON.parse(text) as { accessToken: string };
    result.set(descriptor.deviceId, { accessToken: body.accessToken });
  }
  return result;
}

test("账号注册 → 控制面授权 → Noise 握手 → 加密 Relay 双向业务", async () => {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "e2e-secret-that-is-long-enough",
    githubVerifier: async () => ({ providerUserId: "99", login: "e2e" }),
  });
  const hostIdentity = generateIdentity();
  const clientIdentity = generateIdentity();
  const host: DeviceDescriptor = {
    deviceId: HOST_ID,
    name: "E2E Host",
    role: "host",
    platform: "desktop",
    identityKey: hostIdentity.identityKey,
    clientVersion: "0.0.1",
  };
  const client: DeviceDescriptor = {
    deviceId: CLIENT_ID,
    name: "E2E Client",
    role: "client",
    platform: "desktop",
    identityKey: clientIdentity.identityKey,
    clientVersion: "0.0.1",
  };
  const hostWire = new Wire(`ws://127.0.0.1:${server.port}/ws/v1/connect`);
  const clientWire = new Wire(`ws://127.0.0.1:${server.port}/ws/v1/connect`);
  let routeClosed = false;
  try {
    const credentials = await loginAndRegister(server, [host, client]);
    await Promise.all([hostWire.open(), clientWire.open()]);
    hostWire.send(
      frame("hello", {
        role: "host",
        deviceId: HOST_ID,
        accessToken: credentials.get(HOST_ID)!.accessToken,
        protocols: [1],
        capabilities: ["secure.noise-ik.v1", "transport.relay"],
      }),
    );
    clientWire.send(
      frame("hello", {
        role: "client",
        deviceId: CLIENT_ID,
        accessToken: credentials.get(CLIENT_ID)!.accessToken,
        protocols: [1],
        capabilities: ["secure.noise-ik.v1", "transport.relay"],
      }),
    );
    await Promise.all([
      hostWire.next((value) => value.type === "hello.ack"),
      clientWire.next((value) => value.type === "hello.ack"),
    ]);

    clientWire.send(
      frame("connect.request", {
        hostDeviceId: HOST_ID,
        preferredTransports: ["relay"],
      }),
    );
    const incoming = await hostWire.next((value) => value.type === "connect.incoming");
    const accepted = await clientWire.next((value) => value.type === "connect.accepted");
    assert.equal(incoming.type, "connect.incoming");
    assert.equal(accepted.type, "connect.accepted");
    if (incoming.type !== "connect.incoming" || accepted.type !== "connect.accepted") return;
    const connectionId = incoming.payload.connectionId;
    assert.equal(accepted.payload.connectionId, connectionId);
    assert.ok(incoming.payload.relay);
    assert.ok(accepted.payload.relay);

    const hostBusiness: BusinessFrame[] = [];
    const clientBusiness: BusinessFrame[] = [];
    let hostReadyResolve: () => void = () => undefined;
    let clientReadyResolve: () => void = () => undefined;
    const hostReady = new Promise<void>((resolve) => {
      hostReadyResolve = resolve;
    });
    const clientReady = new Promise<void>((resolve) => {
      clientReadyResolve = resolve;
    });

    const hostSession = new SecureSession({
      role: "host",
      localIdentity: hostIdentity,
      peerIdentityKey: clientIdentity.identityKey,
      context: { connectionId, hostDeviceId: HOST_ID, clientDeviceId: CLIENT_ID },
      sendHandshake: (step, data) =>
        hostWire.send(
          frame("secure.handshake", {
            connectionId,
            targetDeviceId: CLIENT_ID,
            step,
            data: encodeBase64Url(data),
          }),
        ),
      onFrame: (value) => hostBusiness.push(value),
      onReady: hostReadyResolve,
      onError: (error) => assert.fail(error.message),
      onClose: () => undefined,
    });
    const clientSession = new SecureSession({
      role: "client",
      localIdentity: clientIdentity,
      peerIdentityKey: hostIdentity.identityKey,
      context: { connectionId, hostDeviceId: HOST_ID, clientDeviceId: CLIENT_ID },
      sendHandshake: (step, data) =>
        clientWire.send(
          frame("secure.handshake", {
            connectionId,
            targetDeviceId: HOST_ID,
            step,
            data: encodeBase64Url(data),
          }),
        ),
      onFrame: (value) => clientBusiness.push(value),
      onReady: clientReadyResolve,
      onError: (error) => assert.fail(error.message),
      onClose: () => undefined,
    });

    const hostRelay = new Wire(incoming.payload.relay.endpoint);
    const clientRelay = new Wire(accepted.payload.relay.endpoint);
    await Promise.all([hostRelay.open(), clientRelay.open()]);
    hostRelay.send(
      frame("relay.attach", {
        connectionId,
        deviceId: HOST_ID,
        peerDeviceId: CLIENT_ID,
        ticket: incoming.payload.relay.ticket,
      }),
    );
    clientRelay.send(
      frame("relay.attach", {
        connectionId,
        deviceId: CLIENT_ID,
        peerDeviceId: HOST_ID,
        ticket: accepted.payload.relay.ticket,
      }),
    );
    await Promise.all([
      hostRelay.next((value) => value.type === "relay.ready"),
      clientRelay.next((value) => value.type === "relay.ready"),
    ]);

    const routeHandshake = async (wire: Wire, session: SecureSession): Promise<void> => {
      while (!routeClosed) {
        try {
          const next = await wire.next((value) => value.type === "secure.handshake");
          if (next.type === "secure.handshake") {
            session.handleHandshake(
              next.payload.step,
              decodeBase64Url(next.payload.data),
            );
          }
        } catch {
          return;
        }
      }
    };
    void routeHandshake(hostWire, hostSession);
    void routeHandshake(clientWire, clientSession);

    let hostCounter = 0;
    let clientCounter = 0;
    const relayedCiphertexts: string[] = [];
    hostRelay.ws.on("message", (data) => {
      const decoded = decodeControlFrame(
        Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer),
      );
      if (decoded.type === "relay") {
        relayedCiphertexts.push(decoded.payload.ciphertext);
        hostSession.handleData(decodeBase64Url(decoded.payload.ciphertext));
      }
    });
    clientRelay.ws.on("message", (data) => {
      const decoded = decodeControlFrame(
        Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer),
      );
      if (decoded.type === "relay") {
        clientSession.handleData(decodeBase64Url(decoded.payload.ciphertext));
      }
    });

    hostSession.attachData((bytes) =>
      hostRelay.send(
        frame("relay", {
          connectionId,
          targetDeviceId: CLIENT_ID,
          counter: hostCounter++,
          ciphertext: encodeBase64Url(bytes),
        }),
      ),
    );
    clientSession.attachData((bytes) =>
      clientRelay.send(
        frame("relay", {
          connectionId,
          targetDeviceId: HOST_ID,
          counter: clientCounter++,
          ciphertext: encodeBase64Url(bytes),
        }),
      ),
    );

    clientWire.send(frame("transport.selected", { connectionId, transport: "relay" }));
    clientSession.start();
    await Promise.all([hostReady, clientReady]);

    clientSession.send({
      v: 1,
      type: "rpc.request",
      id: "req-1",
      method: "remote.echo",
      params: { value: "secret-payload" },
    });
    await waitFor(() => hostBusiness.length > 0);
    assert.deepEqual(hostBusiness[0], {
      v: 1,
      type: "rpc.request",
      id: "req-1",
      method: "remote.echo",
      params: { value: "secret-payload" },
    });

    hostSession.send({ v: 1, type: "rpc.response", id: "req-1", result: { ok: true } });
    await waitFor(() => clientBusiness.length > 0);
    assert.deepEqual(clientBusiness[0], {
      v: 1,
      type: "rpc.response",
      id: "req-1",
      result: { ok: true },
    });

    assert.ok(relayedCiphertexts.length > 0);
    for (const ciphertext of relayedCiphertexts) {
      const raw = Buffer.from(ciphertext, "base64url").toString("latin1");
      assert.equal(raw.includes("remote.echo"), false);
      assert.equal(raw.includes("secret-payload"), false);
    }

    hostRelay.close();
    clientRelay.close();
    routeClosed = true;
  } finally {
    routeClosed = true;
    hostWire.close();
    clientWire.close();
    await server.close();
  }
});

async function waitFor(predicate: () => boolean, timeoutMs = 3000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("等待业务帧超时");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}
