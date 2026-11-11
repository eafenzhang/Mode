import assert from "node:assert/strict";
import test from "node:test";

import { generateIdentity } from "../src/adapters/identity.js";
import {
  decodeControlFrame,
  encodeControlFrame,
  type ControlFrame,
} from "../src/domain/controlFrames.js";
import { RemoteError } from "../src/domain/errors.js";
import { decodeBusinessFrame, encodeBusinessFrame } from "../src/domain/businessFrames.js";
import {
  decodeFragmentHeader,
  FragmentReassembler,
  fragmentMessage,
} from "../src/domain/fragments.js";

const identity = generateIdentity();

function hello(): ControlFrame {
  return {
    v: 1,
    id: "msg-1",
    timestamp: Date.now(),
    type: "hello",
    payload: {
      role: "host",
      deviceId: "019a0000-0000-7000-8000-000000000001",
      accessToken: "token",
      protocols: [1],
      capabilities: ["secure.noise-ik.v1"],
    },
  };
}

test("control frame 严格往返并拒绝未知字段", () => {
  const frame = hello();
  const bytes = encodeControlFrame(frame);
  assert.deepEqual(decodeControlFrame(bytes), frame);

  const withUnknown = JSON.parse(Buffer.from(bytes).toString("utf8"));
  withUnknown.payload.extra = true;
  assert.throws(
    () => decodeControlFrame(Buffer.from(JSON.stringify(withUnknown))),
    (error: unknown) => error instanceof RemoteError && error.code === "invalid_message",
  );
});

test("control frame 拒绝非规范 base64url 与错误版本", () => {
  const frame: ControlFrame = {
    v: 1,
    id: "msg-2",
    timestamp: 1,
    type: "secure.handshake",
    payload: {
      connectionId: "c1",
      targetDeviceId: "d1",
      step: 1,
      data: "abc=",
    },
  };
  assert.throws(() => decodeControlFrame(Buffer.from(JSON.stringify(frame))), RemoteError);
});

test("业务帧分片可往返并拒绝乱序/重复", () => {
  const plaintext = Buffer.alloc(200_000, 7);
  const fragments = fragmentMessage(42, plaintext);
  assert.equal(fragments.length, 5);
  assert.equal(decodeFragmentHeader(fragments[0]!).messageId, 42);
  const reassembler = new FragmentReassembler();
  let result: Uint8Array | null = null;
  for (const fragment of fragments) {
    const message = reassembler.push(fragment);
    if (message) result = message.plaintext;
  }
  assert.deepEqual(result, plaintext);

  const duplicate = new FragmentReassembler();
  duplicate.push(fragments[0]!);
  assert.throws(() => duplicate.push(fragments[0]!), RemoteError);

  const outOfOrder = new FragmentReassembler();
  assert.throws(() => outOfOrder.push(fragments[1]!), RemoteError);
});

test("业务帧严格解析", () => {
  const frame = {
    v: 1 as const,
    type: "rpc.request" as const,
    id: "r1",
    method: "remote.echo",
    params: { value: 1 },
  };
  assert.deepEqual(decodeBusinessFrame(encodeBusinessFrame(frame)), frame);
  const invalid = { ...frame, extra: true };
  assert.throws(() => decodeBusinessFrame(encodeBusinessFrame(invalid as never)), RemoteError);
});
