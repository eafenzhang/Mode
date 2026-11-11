/**
 * 浏览器 v3 主控远程桌面：会话创建走 HTTP 桥、信令/媒体走 remote.ws 隧道。
 *
 * 断言三层契约：
 * 1. `POST /api/remote-desktop/session` 的路径参数与票据解析；
 * 2. 信令帧的隧道封装（首字节 0=text、1=binary）与双向 schema 校验；
 * 3. 媒体 socket 契约（onopen/onmessage/onclose、文本与二进制上行）。
 */
import assert from "node:assert/strict";
import test from "node:test";

import type { WebBusinessStream } from "@mode/remote-core/web";
import {
  createWebRemoteDesktopSession,
  type WebRemoteDesktopSession,
} from "@mode/client";
import type { RemoteDesktopSessionEvent } from "@mode/shared/remote-desktop";

interface FakeStream extends WebBusinessStream {
  sent: Uint8Array[];
  emitData(bytes: Uint8Array): void;
  emitClose(reason?: string): void;
  closedWith: string[];
}

interface OpenedStream {
  service: string;
  metadata: Record<string, unknown>;
  stream: FakeStream;
}

function createFakeStream(streamId: number): FakeStream {
  const dataListeners = new Set<(bytes: Uint8Array) => void>();
  const closeListeners = new Set<(reason: string) => void>();
  const stream: FakeStream = {
    streamId,
    sent: [],
    closedWith: [],
    send(bytes) {
      stream.sent.push(bytes);
    },
    close(reason = "closed") {
      stream.closedWith.push(reason);
      for (const listener of closeListeners) listener(reason);
    },
    onData(callback) {
      dataListeners.add(callback);
      return () => dataListeners.delete(callback);
    },
    onClose(callback) {
      closeListeners.add(callback);
      return () => closeListeners.delete(callback);
    },
    emitData(bytes) {
      for (const listener of dataListeners) listener(bytes);
    },
    emitClose(reason = "peer closed") {
      for (const listener of closeListeners) listener(reason);
    },
  };
  return stream;
}

/** Node 没有 WebCodecs：适配层预检按浏览器能力判断，这里注入/撤销占位实现。 */
function setVideoDecoder(value: unknown): void {
  (globalThis as Record<string, unknown>).VideoDecoder = value;
}

setVideoDecoder(class {});

function encodeBody(value: unknown): string {
  return Buffer.from(JSON.stringify(value), "utf8").toString("base64url");
}

function textFrame(value: unknown): Uint8Array {
  const payload = new TextEncoder().encode(JSON.stringify(value));
  const frame = new Uint8Array(payload.length + 1);
  frame[0] = 0;
  frame.set(payload, 1);
  return frame;
}

function decodeTextFrame(frame: Uint8Array): unknown {
  assert.equal(frame[0], 0, "信令上行必须是 text 帧");
  return JSON.parse(new TextDecoder().decode(frame.subarray(1)));
}

function createHarness(options: { sessionStatus?: number; sessionBody?: unknown } = {}) {
  const opened: OpenedStream[] = [];
  const requests: { method: string; params: Record<string, unknown> }[] = [];
  let nextStreamId = 1;
  const link = {
    async request(method: string, params: Record<string, unknown>) {
      requests.push({ method, params });
      const status = options.sessionStatus ?? 200;
      const body = options.sessionBody ?? { ticket: "signal-1", mediaTicket: "media-1" };
      return { status, headers: {}, bodyBase64: encodeBody(body) };
    },
    openStream(service: string, metadata: Record<string, unknown>) {
      const stream = createFakeStream(nextStreamId++);
      opened.push({ service, metadata, stream });
      return stream;
    },
  };
  const events: RemoteDesktopSessionEvent[] = [];
  const session: WebRemoteDesktopSession = createWebRemoteDesktopSession({
    link,
    hostDeviceId: "host-1",
    quality: "sharp",
    codec: "h264",
    onEvent: (event) => events.push(event),
  });
  return { session, link, events, opened, requests };
}

async function tick(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

test("v3 主控：HTTP 桥创建会话并返回虚拟信令/媒体地址", async () => {
  const harness = createHarness();
  const result = await harness.session.start();
  assert.equal(result.ok, true);
  assert.ok(result.ok && result.mediaUrl?.includes("mediaTicket=media-1"));

  assert.deepEqual(harness.requests, [
    {
      method: "remote.http.request",
      params: {
        method: "POST",
        path: "/api/remote-desktop/session?quality=sharp&codec=h264",
      },
    },
  ]);
  const signal = harness.opened[0]!;
  assert.equal(signal.service, "remote.ws");
  assert.equal(signal.metadata.path, "/ws/remote-desktop?ticket=signal-1");

  await tick();
  assert.ok(
    harness.events.some((event) => event.kind === "state" && event.state === "connected"),
    "隧道打开后推出 connected",
  );
});

test("v3 主控：信令上行 text 帧、下行经 schema 校验", async () => {
  const harness = createHarness();
  await harness.session.start();
  await tick();
  const signal = harness.opened[0]!.stream;

  harness.session.sendSignal({ type: "select-monitor", displayId: "display-1" });
  assert.deepEqual(decodeTextFrame(signal.sent[0]!), {
    type: "select-monitor",
    displayId: "display-1",
  });

  signal.emitData(
    textFrame({
      type: "media-open",
      key: "a".repeat(64),
      salt: "b".repeat(16),
    }),
  );
  const open = harness.events.find((event) => event.kind === "signal");
  assert.deepEqual(open, {
    kind: "signal",
    message: { type: "media-open", key: "a".repeat(64), salt: "b".repeat(16) },
  });
});

test("v3 主控：媒体 socket 契约（二进制下行/文本与二进制上行）", async () => {
  const harness = createHarness();
  const result = await harness.session.start();
  assert.ok(result.ok && result.mediaUrl);

  const media = harness.session.openMediaSocket(result.mediaUrl);
  await tick();
  assert.equal(media.readyState, 1);
  const mediaStream = harness.opened[1]!.stream;
  assert.equal(harness.opened[1]!.metadata.path, "/ws/remote-desktop-media?mediaTicket=media-1");

  const received: unknown[] = [];
  media.onmessage = (event) => received.push(event.data);
  const binary = new Uint8Array([9, 8, 7]);
  const frame = new Uint8Array(binary.length + 1);
  frame[0] = 1;
  frame.set(binary, 1);
  mediaStream.emitData(frame);
  assert.deepEqual(Array.from(new Uint8Array(received[0] as ArrayBuffer)), [9, 8, 7]);

  media.send('{"t":"ready"}');
  assert.deepEqual(decodeTextFrame(mediaStream.sent[0]!), { t: "ready" });
  media.send(binary);
  assert.equal(mediaStream.sent[1]![0], 1, "二进制上行为 binary 帧");
  assert.deepEqual(Array.from(mediaStream.sent[1]!.subarray(1)), [9, 8, 7]);
});

test("v3 主控：缺少 WebCodecs 时本机预检拒绝，不请求对端", async () => {
  setVideoDecoder(undefined);
  try {
    const harness = createHarness();
    assert.deepEqual(await harness.session.start(), { error: "decoder_unsupported" });
    assert.equal(harness.requests.length, 0, "预检失败不应发起 HTTP 请求");
    assert.ok(
      harness.events.some(
        (event) =>
          event.kind === "state" &&
          event.state === "closed" &&
          event.error === "decoder_unsupported",
      ),
    );
  } finally {
    setVideoDecoder(class {});
  }
});

test("v3 主控：对端拒绝时把错误码交给会话层", async () => {
  const harness = createHarness({ sessionStatus: 403, sessionBody: { code: "consent_denied" } });
  assert.deepEqual(await harness.session.start(), { error: "consent_denied" });
  assert.ok(
    harness.events.some(
      (event) => event.kind === "state" && event.state === "closed" && event.error === "consent_denied",
    ),
  );
});
