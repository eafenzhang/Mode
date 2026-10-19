/**
 * LR-2：`remote-desktop` 被控设置面的端口往返（生产 Host + 真 MessagePort）。
 *
 * 不启动任何会话：只验证频道注册、真实 server 状态读取、关闭态写回、空 clientId 拒绝、
 * 被控状态与对端能力探测的默认值。会话链路在 LR-3 的测试里覆盖。
 */
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import assert from "node:assert/strict";
import test from "node:test";

import type { IChannel } from "@mode/rpc";
import type {
  RemoteDesktopServerState,
  RemoteDesktopSessionEvent,
} from "@mode/shared/remote-desktop";

import {
  callWithTimeout,
  connectProductionHost,
  describeHost,
  disposeHost,
  removeSandbox,
} from "../../../apps/desktop-tauri/test/fixtures/productionHost.mts";

test("生产 Host 注册 remote-desktop：设置面可读、关闭写回、空 clientId 拒绝", async () => {
  const root = mkdtempSync(join(tmpdir(), "mode-rd-port-"));
  const home = join(root, "home");
  const dataBaseDir = join(root, "data");
  const host = await connectProductionHost({ home, dataBaseDir });
  try {
    const describe = await describeHost(host);
    assert.ok(
      describe.serviceChannels.includes("remote-desktop"),
      `Host 必须注册 remote-desktop 频道，实际：${describe.serviceChannels.join(",")}`,
    );

    const channel = host.client.getChannel<IChannel>("remote-desktop");
    const state = await callWithTimeout(
      channel.call<RemoteDesktopServerState>("getServerState"),
      "remote-desktop.getServerState",
    );
    assert.equal(state.enabled, false, "隔离数据根下被控开关默认关");
    assert.deepEqual(state.authorizedClients, []);
    assert.equal(state.encoderCapability, "unknown", "没有真实会话观测时不猜编码能力");

    const controlled = await callWithTimeout(
      channel.call<{ controlled: boolean }>("getControlledState"),
      "remote-desktop.getControlledState",
    );
    assert.equal(controlled.controlled, false);

    // 订阅后写一次关闭态：必须收到状态快照（幂等事件）。
    const events: RemoteDesktopServerState[] = [];
    const event = channel.listen<RemoteDesktopServerState>("onDidChangeServerState");
    const subscription = event((next) => events.push(next));
    const written = await callWithTimeout(
      channel.call<RemoteDesktopServerState>("setServerEnabled", [{ enabled: false }]),
      "remote-desktop.setServerEnabled",
    );
    assert.equal(written.enabled, false);
    await new Promise((resolve) => setTimeout(resolve, 100));
    subscription.dispose();
    assert.ok(events.length >= 1, "写开关必须广播状态快照");
    assert.equal(events.at(-1)?.enabled, false);

    await assert.rejects(
      callWithTimeout(
        channel.call("authorizeClient", [{ clientId: " " }]),
        "remote-desktop.authorizeClient",
      ),
      /clientId/u,
    );

    const peer = await callWithTimeout(
      channel.call<Record<string, unknown>>("getPeerCapability", [{ serverId: "" }]),
      "remote-desktop.getPeerCapability",
    );
    assert.deepEqual(peer, {
      remoteDesktop: false,
      remoteDesktopNative: false,
      online: false,
    });

    // LR-3：主控会话经真实 controller 打开对端；未配对时按稳定错误码收口。
    const sessionEvents: RemoteDesktopSessionEvent[] = [];
    const sessionEvent = channel.listen<RemoteDesktopSessionEvent>("onDynamicSessionEvent");
    const sessionSubscription = sessionEvent((next) => sessionEvents.push(next));
    try {
      // 等 EventListen 先到达 Host（与随后的 Call 走同一条有序传输，但订阅请求是微任务发的）。
      await new Promise((resolve) => setTimeout(resolve, 50));
      const startResult = await callWithTimeout(
        channel.call("startSession", [{ serverId: "not-paired" }]),
        "remote-desktop.startSession",
      );
      assert.deepEqual(startResult, { error: "unauthorized" });
      await new Promise((resolve) => setTimeout(resolve, 100));
      assert.deepEqual(
        sessionEvents.map((entry) =>
          entry.kind === "state" ? `${entry.state}:${entry.error ?? ""}` : "signal",
        ),
        ["connecting:", "closed:unauthorized"],
        "动态事件只发当前订阅者，并带稳定错误码",
      );

      // 无会话时信令是静默丢弃；end 是幂等收口。
      await callWithTimeout(
        channel.call("sendSignal", [{ message: { type: "bye" } }]),
        "remote-desktop.sendSignal",
      );
      await callWithTimeout(channel.call("endSession", []), "remote-desktop.endSession");

      await assert.rejects(
        callWithTimeout(
          channel.call("respondConsent", [
            { clientId: "nobody", decision: "allow", remember: false },
          ]),
          "remote-desktop.respondConsent",
        ),
        /没有待处理的同意请求/u,
      );
    } finally {
      sessionSubscription.dispose();
    }
  } finally {
    await disposeHost(host);
    await removeSandbox(root);
  }
});
