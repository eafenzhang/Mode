// P1/T4：扩展 cookie -> Electron session 的映射与端到端写入。
// 命名管道只在 Windows 上跑；映射用例在所有平台跑。
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { connect, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  closeActiveBrowserImportChannels,
  importCookiesFromExtension,
  toElectronCookie,
  type BrowserImportCookieDetails,
} from "../src/main/browserImportExtension.ts";

const isWindows = process.platform === "win32";
const log = { info: () => undefined, warn: () => undefined };

function frame(value: unknown): Buffer {
  const payload = Buffer.from(JSON.stringify(value), "utf8");
  const out = Buffer.allocUnsafe(4 + payload.length);
  out.writeUInt32LE(payload.length, 0);
  payload.copy(out, 4);
  return out;
}

function reader(socket: Socket): { next: () => Promise<Record<string, unknown>> } {
  let buffered = Buffer.alloc(0);
  const frames: Record<string, unknown>[] = [];
  const waiters: Array<(value: Record<string, unknown>) => void> = [];
  socket.on("data", (chunk: Buffer) => {
    buffered = Buffer.concat([buffered, chunk]);
    while (buffered.length >= 4) {
      const length = buffered.readUInt32LE(0);
      if (buffered.length < 4 + length) return;
      const payload = buffered.subarray(4, 4 + length);
      buffered = buffered.subarray(4 + length);
      const value = JSON.parse(payload.toString("utf8")) as Record<string, unknown>;
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

async function waitForFile(path: string): Promise<void> {
  for (let i = 0; i < 100; i += 1) {
    if (await stat(path).then(() => true, () => false)) return;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error(`channel file not created: ${path}`);
}

test("maps domain, host-only, secure, sameSite and expiration", () => {
  const domain = toElectronCookie({
    name: "a",
    value: "1",
    domain: ".example.com",
    path: "/x",
    secure: true,
    httpOnly: true,
    sameSite: "lax",
    expirationDate: 1_900_000_000,
  });
  assert.deepEqual(domain, {
    url: "https://example.com/x",
    name: "a",
    value: "1",
    domain: ".example.com",
    path: "/x",
    secure: true,
    httpOnly: true,
    sameSite: "lax",
    expirationDate: 1_900_000_000,
  });

  const hostOnly = toElectronCookie({
    name: "b",
    value: "2",
    domain: "example.com",
    path: "/",
    secure: false,
    httpOnly: false,
    sameSite: "strict",
  });
  assert.equal(hostOnly?.domain, undefined);
  assert.equal(hostOnly?.url, "http://example.com/");
  assert.equal(hostOnly?.sameSite, "strict");
  assert.equal(hostOnly?.expirationDate, undefined);
});

test("rejects unsafe cookie shapes", () => {
  assert.equal(toElectronCookie({ name: "a", value: "1", domain: "", path: "/" }), null);
  assert.equal(toElectronCookie({ name: "a", value: "1", domain: "example.com", path: "x" }), null);
  assert.equal(toElectronCookie({ name: "a", domain: "example.com", path: "/" }), null);
});

test("extension import writes cookies and flushes", { skip: !isWindows }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-import-ext-"));
  const channelPath = join(dir, "channel.json");
  const writes: BrowserImportCookieDetails[] = [];
  let flushed = false;
  const targetSession = {
    cookies: {
      set: async (details: BrowserImportCookieDetails) => {
        writes.push(details);
      },
      flushStore: async () => {
        flushed = true;
      },
    },
  };

  const pending = importCookiesFromExtension({
    targetSession,
    logger: log,
    channelPath,
    timeoutMs: 5000,
  });

  try {
    await waitForFile(channelPath);
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
          { name: "a", value: "1", domain: "example.com", path: "/" },
          { name: "b", value: "2", domain: ".example.com", path: "/", secure: true },
        ],
      }),
    );
    assert.deepEqual(await incoming.next(), { ok: true, count: 2 });
    const result = await pending;
    assert.equal(result.imported, 2);
    assert.equal(result.error, undefined);
    assert.equal(writes.length, 2);
    assert.equal(flushed, true);
    socket.destroy();
  } finally {
    await pending.catch(() => undefined);
    await rm(dir, { recursive: true, force: true });
  }
});

test("extension import times out without a client", { skip: !isWindows }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-import-ext-"));
  const result = await importCookiesFromExtension({
    targetSession: { cookies: { set: async () => undefined, flushStore: async () => undefined } },
    logger: log,
    channelPath: join(dir, "channel.json"),
    timeoutMs: 100,
  });
  assert.equal(result.imported, 0);
  assert.equal(result.error, "channel_timeout");
  await rm(dir, { recursive: true, force: true });
});

test("wrong token fails fast with channel_token_mismatch", { skip: !isWindows }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-import-ext-"));
  const channelPath = join(dir, "channel.json");
  const pending = importCookiesFromExtension({
    targetSession: { cookies: { set: async () => undefined, flushStore: async () => undefined } },
    logger: log,
    channelPath,
    timeoutMs: 5000,
  });
  try {
    await waitForFile(channelPath);
    const info = JSON.parse(await readFile(channelPath, "utf8")) as { pipe: string };
    const socket = await connected(info.pipe);
    const incoming = reader(socket);
    socket.write(frame({ v: 1, kind: "hello", token: "wrong" }));
    assert.deepEqual(await incoming.next(), { ok: false, error: "channel_token_mismatch" });
    const result = await pending;
    assert.equal(result.error, "channel_token_mismatch");
    socket.destroy();
  } finally {
    await pending.catch(() => undefined);
    await rm(dir, { recursive: true, force: true });
  }
});

test("invalid payload fails fast without writing", { skip: !isWindows }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-import-ext-"));
  const channelPath = join(dir, "channel.json");
  let writes = 0;
  const pending = importCookiesFromExtension({
    targetSession: {
      cookies: {
        set: async () => {
          writes += 1;
        },
        flushStore: async () => undefined,
      },
    },
    logger: log,
    channelPath,
    timeoutMs: 5000,
  });
  try {
    await waitForFile(channelPath);
    const info = JSON.parse(await readFile(channelPath, "utf8")) as {
      pipe: string;
      token: string;
    };
    const socket = await connected(info.pipe);
    const incoming = reader(socket);
    socket.write(frame({ v: 1, kind: "hello", token: info.token }));
    assert.deepEqual(await incoming.next(), { ok: true });
    socket.write(frame({ v: 1, kind: "cookies", cookies: [{ name: 1 }] }));
    assert.deepEqual(await incoming.next(), { ok: false, error: "invalid_payload" });
    const result = await pending;
    assert.equal(result.error, "invalid_payload");
    assert.equal(writes, 0);
    socket.destroy();
  } finally {
    await pending.catch(() => undefined);
    await rm(dir, { recursive: true, force: true });
  }
});

test("quitting closes waiting channels and removes channel.json", { skip: !isWindows }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "mode-import-ext-"));
  const channelPath = join(dir, "channel.json");
  const pending = importCookiesFromExtension({
    targetSession: { cookies: { set: async () => undefined, flushStore: async () => undefined } },
    logger: log,
    channelPath,
    timeoutMs: 300,
  });
  try {
    await waitForFile(channelPath);
    closeActiveBrowserImportChannels();
    await assert.rejects(stat(channelPath));
    const result = await pending;
    assert.equal(result.error, "channel_timeout");
  } finally {
    await pending.catch(() => undefined);
    await rm(dir, { recursive: true, force: true });
  }
});
