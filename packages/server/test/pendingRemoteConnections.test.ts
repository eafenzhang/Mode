/**
 * 待接管远程连接的生命周期（docs/specs/web-remote-workspace-ssh-docker.md §4）：
 * 60s TTL、并发上限、take 后不再回收、服务关闭全量释放。
 */
import assert from "node:assert/strict";
import test from "node:test";

import type { RemoteConnection } from "../src/remote/connect.js";
import { PendingRemoteConnections } from "../src/remote/pendingRemoteConnections.js";

function fakeConnection(): { connection: RemoteConnection; isDisposed: () => boolean } {
  let disposed = false;
  return {
    connection: {
      services: {} as RemoteConnection["services"],
      client: {} as RemoteConnection["client"],
      dispose: () => {
        disposed = true;
      },
      disposeAndWait: async () => {
        disposed = true;
      },
    },
    isDisposed: () => disposed,
  };
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

test("take 取出即从待接管表删除", () => {
  const pending = new PendingRemoteConnections();
  const fake = fakeConnection();
  const added = pending.add(fake.connection);
  assert.equal(added.ok, true);
  if (!added.ok) {
    return;
  }
  assert.equal(pending.size, 1);
  assert.equal(pending.take(added.id), fake.connection);
  assert.equal(pending.take(added.id), null);
  assert.equal(pending.size, 0);
  assert.equal(fake.isDisposed(), false, "接管后由 WS close 释放，不由本表释放");
});

test("超过 TTL 未接管自动 dispose", async () => {
  const pending = new PendingRemoteConnections({ ttlMs: 20 });
  const fake = fakeConnection();
  const added = pending.add(fake.connection);
  assert.equal(added.ok, true);
  await delay(80);
  assert.equal(fake.isDisposed(), true);
  assert.equal(pending.size, 0);
});

test("并发上限：超出时拒绝且不接管已有条目", () => {
  const pending = new PendingRemoteConnections({ maxPending: 1 });
  const first = fakeConnection();
  const second = fakeConnection();
  assert.equal(pending.add(first.connection).ok, true);
  const rejected = pending.add(second.connection);
  assert.deepEqual(rejected, { ok: false, error: "too_many_pending_connections" });
  assert.equal(second.isDisposed(), false, "拒绝的调用方负责 dispose");
  assert.equal(pending.size, 1);
});

test("disposeAll 释放全部待接管连接", () => {
  const pending = new PendingRemoteConnections();
  const first = fakeConnection();
  const second = fakeConnection();
  assert.equal(pending.add(first.connection).ok, true);
  assert.equal(pending.add(second.connection).ok, true);
  pending.disposeAll();
  assert.equal(first.isDisposed(), true);
  assert.equal(second.isDisposed(), true);
  assert.equal(pending.size, 0);
});
