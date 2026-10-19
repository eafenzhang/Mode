/**
 * LR-1：`lan-access` 频道的**端口往返**用例——起生产 Host sidecar，用真 `MessagePort` +
 * `ChannelClient` 调用，覆盖「频道确实注册了」「事件能过桥」「失败是拒绝而不是假成功」。
 *
 * 这里不启动监听（设置默认关闭，测试机不绑定 45880/45879）：启动/停止的语义由
 * `lan-access-host-service.test.mts` 用假 server 覆盖。本用例只守线上契约。
 */
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import assert from "node:assert/strict";
import test from "node:test";

import type { IChannel } from "@mode/rpc";
import type { LanAccessState } from "@mode/shared";

import {
  callWithTimeout,
  connectProductionHost,
  describeHost,
  disposeHost,
  removeSandbox,
} from "../../../apps/desktop-tauri/test/fixtures/productionHost.mts";

test("生产 Host 注册 lan-access：状态可读、事件可达、未开启的动作明确拒绝", async () => {
  const root = mkdtempSync(join(tmpdir(), "mode-lan-port-"));
  const home = join(root, "home");
  const dataBaseDir = join(root, "data");
  const host = await connectProductionHost({ home, dataBaseDir });
  try {
    const describe = await describeHost(host);
    assert.ok(
      describe.serviceChannels.includes("lan-access"),
      `Host 必须注册 lan-access 频道，实际：${describe.serviceChannels.join(",")}`,
    );

    const channel = host.client.getChannel<IChannel>("lan-access");
    const state = await callWithTimeout(
      channel.call<LanAccessState>("getState"),
      "lan-access.getState",
    );
    assert.equal(state.enabled, false);
    assert.equal(state.port, null);
    assert.deepEqual(state.clients, []);

    // 订阅幂等快照：禁用动作（即使本来就没开）也要发一次最新状态。
    const event = channel.listen<LanAccessState>("onDidChange");
    const emitted = new Promise<LanAccessState>((resolve) => {
      const disposable = event((next) => {
        disposable.dispose();
        resolve(next);
      });
    });
    await callWithTimeout(
      channel.call("setEnabled", [{ enabled: false }]),
      "lan-access.setEnabled(false)",
    );
    const pushed = await callWithTimeout(emitted, "lan-access.onDidChange");
    assert.equal(pushed.enabled, false);

    // 未开启监听时，配对码必须明确拒绝（不能返回 enabled:false 的假成功）。
    await assert.rejects(
      callWithTimeout(channel.call("createPairCode"), "lan-access.createPairCode"),
      /未开启/u,
    );

    // 这两个曾是可被读成「操作成功」的静默 no-op：现在缺参数要真实拒绝。
    await assert.rejects(
      callWithTimeout(
        channel.call("removePairedPeer", [{ serverId: "" }]),
        "lan-access.removePairedPeer",
      ),
      /缺少 serverId/u,
    );
    await assert.rejects(
      callWithTimeout(
        channel.call("renamePairedPeer", [{ serverId: "", name: "x" }]),
        "lan-access.renamePairedPeer",
      ),
      /缺少 serverId/u,
    );
  } finally {
    await disposeHost(host);
    await removeSandbox(root);
  }
});
