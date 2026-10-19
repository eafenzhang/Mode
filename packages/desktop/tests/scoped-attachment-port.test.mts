/**
 * LR-4：scoped attachment 的生产 Host 端口往返。
 *
 * 起生产 Host（`MODE_HOST_TEST_SCOPES=1` 打开诊断入口），经页面端口的同一套多路复用
 * 建两个 scope，验证：
 * 1. 两个 scope 的 RPC 并发不串台；base 频道路由不受影响；
 * 2. 一个 scope detach 后只回收自己，另一个仍可用，旧 scope 的请求不再被应答；
 * 3. 生产桥的 base 帧格式没有变化（`describeHost` 仍走 base 频道）。
 */
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import assert from "node:assert/strict";
import test from "node:test";

import type { IChannel } from "@mode/rpc";

import {
  callWithTimeout,
  connectProductionHost,
  describeHost,
  disposeHost,
  removeSandbox,
} from "../../../apps/desktop-tauri/test/fixtures/productionHost.mts";

interface ScopeEcho {
  scopeId: number;
  arg: unknown;
}

test("生产 Host：两个 scope 并发多路复用，detach 只回收自己", async () => {
  const root = mkdtempSync(join(tmpdir(), "mode-scoped-port-"));
  const home = join(root, "home");
  const dataBaseDir = join(root, "data");
  const host = await connectProductionHost({
    home,
    dataBaseDir,
    extraEnv: { MODE_HOST_TEST_SCOPES: "1" },
  });
  try {
    const hostChannel = host.client.getChannel<IChannel>("host");
    const describe = await describeHost(host);
    assert.ok(describe.serviceChannels.includes("remote"), "生产 Host 必须注册 remote 管理频道");
    const first = await callWithTimeout(
      hostChannel.call<{ scopeId: number }>("attachTestScope"),
      "host.attachTestScope(1)",
    );
    const second = await callWithTimeout(
      hostChannel.call<{ scopeId: number }>("attachTestScope"),
      "host.attachTestScope(2)",
    );
    assert.notEqual(first.scopeId, second.scopeId, "scope id 必须唯一");

    const firstClient = host.createScopedClient(first.scopeId);
    const secondClient = host.createScopedClient(second.scopeId);
    // 页面端口建好后再发 scope-ready（与真实页面桥的回执同序）。
    host.sendControl({ type: "scope-ready", scopeId: first.scopeId });
    host.sendControl({ type: "scope-ready", scopeId: second.scopeId });

    const firstEcho = callWithTimeout(
      firstClient.getChannel<IChannel>("scope-echo").call<ScopeEcho>("echo", { n: 1 }),
      "scope-echo(1)",
    );
    const secondEcho = callWithTimeout(
      secondClient.getChannel<IChannel>("scope-echo").call<ScopeEcho>("echo", { n: 2 }),
      "scope-echo(2)",
    );
    assert.deepEqual(await firstEcho, { scopeId: first.scopeId, arg: { n: 1 } });
    assert.deepEqual(await secondEcho, { scopeId: second.scopeId, arg: { n: 2 } });

    // base 频道不受影响。
    const hostDescription = await describeHost(host);
    assert.equal(hostDescription.name, "mode-host-sidecar");

    // detach 第一个：第二个继续可用；旧 scope 的请求不再被应答。
    await callWithTimeout(
      hostChannel.call("detachTestScope", { scopeId: first.scopeId }),
      "host.detachTestScope",
    );
    assert.deepEqual(
      await callWithTimeout(
        secondClient.getChannel<IChannel>("scope-echo").call<ScopeEcho>("echo", { n: 3 }),
        "scope-echo(2) after detach",
      ),
      { scopeId: second.scopeId, arg: { n: 3 } },
    );
    await assert.rejects(
      callWithTimeout(
        firstClient.getChannel<IChannel>("scope-echo").call("echo", { n: 4 }),
        "scope-echo(1) after detach",
        500,
      ),
      /超时/u,
      "detach 后旧 scope 不应再收到应答",
    );

    // 重复 detach 是明确错误，不是静默成功。
    await assert.rejects(
      callWithTimeout(
        hostChannel.call("detachTestScope", { scopeId: first.scopeId }),
        "host.detachTestScope(again)",
      ),
      /不存在/u,
    );

    assert.match(host.stderr(), /scope_attached/u);
  } finally {
    await disposeHost(host);
    await removeSandbox(root);
  }
});
