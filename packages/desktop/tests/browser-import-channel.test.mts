// P1/T4：Mode main 的浏览器导入命名管道。
//
// 只在 Windows 上跑：通道是 `\\.\pipe\...`（浏览器数据导入本身也是 Windows 专有）。
// 运行：pnpm exec tsx --test packages/desktop/tests/browser-import-channel.test.mts
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { connect, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { createBrowserImportChannel } from "../src/main/browserImportChannel.ts";

const isWindows = process.platform === "win32";

type Frame = Record<string, unknown>;

function frame(value: Frame): Buffer {
  const payload = Buffer.from(JSON.stringify(value), "utf8");
  const out = Buffer.allocUnsafe(4 + payload.length);
  out.writeUInt32LE(payload.length, 0);
  payload.copy(out, 4);
  return out;
}

function reader(socket: Socket): { next: () => Promise<Frame> } {
  let buffered = Buffer.alloc(0);
  const frames: Frame[] = [];
  const waiters: Array<(value: Frame) => void> = [];
  socket.on("data", (chunk: Buffer) => {
    buffered = Buffer.concat([buffered, chunk]);
    while (buffered.length >= 4) {
      const length = buffered.readUInt32LE(0);
      if (buffered.length < 4 + length) return;
      const payload = buffered.subarray(4, 4 + length);
      buffered = buffered.subarray(4 + length);
      const value = JSON.parse(payload.toString("utf8")) as Frame;
      const waiter = waiters.shift();
      if (waiter) waiter(value);
      else frames.push(value);
    }
  });
  return {
    next: () =>
      frames.length > 0
        ? Promise.resolve(frames.shift()!)
        : new Promise((resolve) => {
            waiters.push(resolve);
          }),
  };
}

async function connected(pipe: string): Promise<Socket> {
  return new Promise((resolve, reject) => {
    const socket = connect(pipe);
    socket.once("connect", () => resolve(socket));
    socket.once("error", reject);
  });
}

test("happy path: hello -> ready -> cookies -> count", { skip: !isWindows }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-import-channel-"));
  const channelPath = join(dir, "channel.json");
  const channel = await createBrowserImportChannel({
    channelPath,
    ttlMs: 5000,
    onCookies: (cookies) => cookies.length,
  });
  try {
    const info = JSON.parse(await readFile(channelPath, "utf8")) as {
      pipe: string;
      token: string;
    };
    const socket = await connected(info.pipe);
    const incoming = reader(socket);
    socket.write(frame({ v: 1, kind: "hello", token: info.token }));
    assert.deepEqual(await incoming.next(), { ok: true });
    socket.write(
      frame({
        v: 1,
        kind: "cookies",
        cookies: [
          { name: "a", value: "1", domain: ".example.com", path: "/" },
          { name: "b", value: "2", domain: ".example.com", path: "/" },
        ],
      }),
    );
    assert.deepEqual(await incoming.next(), { ok: true, count: 2 });
    socket.destroy();
  } finally {
    await channel.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test("wrong token is rejected", { skip: !isWindows }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-import-channel-"));
  const channelPath = join(dir, "channel.json");
  const channel = await createBrowserImportChannel({
    channelPath,
    ttlMs: 5000,
    onCookies: () => 0,
  });
  try {
    const info = JSON.parse(await readFile(channelPath, "utf8")) as { pipe: string };
    const socket = await connected(info.pipe);
    const incoming = reader(socket);
    socket.write(frame({ v: 1, kind: "hello", token: "wrong" }));
    assert.deepEqual(await incoming.next(), { ok: false, error: "channel_token_mismatch" });
    socket.destroy();
  } finally {
    await channel.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test("invalid cookie payload is rejected", { skip: !isWindows }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-import-channel-"));
  const channelPath = join(dir, "channel.json");
  const channel = await createBrowserImportChannel({
    channelPath,
    ttlMs: 5000,
    onCookies: () => 0,
  });
  try {
    const info = JSON.parse(await readFile(channelPath, "utf8")) as {
      pipe: string;
      token: string;
    };
    const socket = await connected(info.pipe);
    const incoming = reader(socket);
    socket.write(frame({ v: 1, kind: "hello", token: info.token }));
    await incoming.next();
    socket.write(frame({ v: 1, kind: "cookies", cookies: [{ name: 1 }] }));
    assert.deepEqual(await incoming.next(), { ok: false, error: "invalid_payload" });
    socket.destroy();
  } finally {
    await channel.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test("ttl closes the channel and removes channel.json", { skip: !isWindows }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-import-channel-"));
  const channelPath = join(dir, "channel.json");
  const errors: string[] = [];
  const channel = await createBrowserImportChannel({
    channelPath,
    ttlMs: 100,
    onCookies: () => 0,
    onError: (code) => errors.push(code),
  });
  await new Promise((resolve) => setTimeout(resolve, 300));
  await assert.rejects(stat(channelPath));
  assert.deepEqual(errors, ["channel_timeout"]);
  await channel.close();
  await rm(dir, { recursive: true, force: true });
});

test("reports terminal errors to the caller", { skip: !isWindows }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-import-channel-"));
  const channelPath = join(dir, "channel.json");
  const errors: string[] = [];
  const channel = await createBrowserImportChannel({
    channelPath,
    ttlMs: 5000,
    onCookies: () => 0,
    onError: (code) => errors.push(code),
  });
  try {
    const info = JSON.parse(await readFile(channelPath, "utf8")) as { pipe: string };
    const socket = await connected(info.pipe);
    const incoming = reader(socket);
    socket.write(frame({ v: 1, kind: "hello", token: "wrong" }));
    assert.deepEqual(await incoming.next(), { ok: false, error: "channel_token_mismatch" });
    assert.deepEqual(errors, ["channel_token_mismatch"]);
    socket.destroy();
  } finally {
    await channel.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test("oversized frame is reported as payload_too_large", { skip: !isWindows }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-import-channel-"));
  const channelPath = join(dir, "channel.json");
  const errors: string[] = [];
  const channel = await createBrowserImportChannel({
    channelPath,
    ttlMs: 5000,
    onCookies: () => 0,
    onError: (code) => errors.push(code),
  });
  try {
    const info = JSON.parse(await readFile(channelPath, "utf8")) as { pipe: string };
    const socket = await connected(info.pipe);
    const incoming = reader(socket);
    const header = Buffer.alloc(4);
    header.writeUInt32LE(2 * 1024 * 1024, 0);
    socket.write(header);
    assert.deepEqual(await incoming.next(), { ok: false, error: "payload_too_large" });
    assert.deepEqual(errors, ["payload_too_large"]);
    socket.destroy();
  } finally {
    await channel.close();
    await rm(dir, { recursive: true, force: true });
  }
});
