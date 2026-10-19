/**
 * LR-3 被控协调器：hello/select-monitor/media-open/MediaStart/freeze/unfreeze 的纯协调测试。
 * 不碰真实 media helper；验证的是协议顺序与稳定错误码。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  REMOTE_DESKTOP_PROTOCOL_VERSION,
  type RemoteDesktopLocalDisplay,
  type RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";
import type { RemoteDesktopMessageToHost } from "../src/host/remoteDesktopMessages.js";
import { RemoteDesktopHostMessageTypes } from "../src/host/remoteDesktopMessages.js";
import { createRemoteDesktopControlledCoordinator } from "../src/host/remoteDesktopControlledCoordinator.js";

const DISPLAYS: RemoteDesktopLocalDisplay[] = [
  {
    displayId: "0",
    label: "Primary",
    xPx: 0,
    yPx: 0,
    widthPx: 1920,
    heightPx: 1080,
    scaleFactor: 1,
    primary: true,
  },
  {
    displayId: "1",
    label: "Secondary",
    xPx: -1280,
    yPx: 0,
    widthPx: 1280,
    heightPx: 1024,
    scaleFactor: 1,
    primary: false,
  },
];

function createCoordinator(displays: RemoteDesktopLocalDisplay[] = DISPLAYS) {
  const signals: RemoteDesktopSignalMessage[] = [];
  const hostMessages: RemoteDesktopMessageToHost[] = [];
  const coordinator = createRemoteDesktopControlledCoordinator({
    getDisplays: () => displays,
    sendSignal: (message) => signals.push(message),
    sendHostMessage: (message) => hostMessages.push(message),
    logger: { info: () => undefined, warn: () => undefined },
    createKeySalt: () => ({ key: "a".repeat(64), salt: "b".repeat(16) }),
  });
  return { coordinator, signals, hostMessages };
}

test("hello → monitors（含 primary、物理尺寸与顺序）", () => {
  const { coordinator, signals } = createCoordinator();
  coordinator.handleControllerSignal({
    type: "hello",
    protocolVersion: REMOTE_DESKTOP_PROTOCOL_VERSION,
  });
  assert.deepEqual(signals, [
    {
      type: "monitors",
      monitors: [
        {
          displayId: "0",
          label: "Primary",
          widthPx: 1920,
          heightPx: 1080,
          scaleFactor: 1,
          primary: true,
        },
        {
          displayId: "1",
          label: "Secondary",
          widthPx: 1280,
          heightPx: 1024,
          scaleFactor: 1,
          primary: false,
        },
      ],
    },
  ]);
});

test("select-monitor：首次 media-open + MediaStart，换屏只发 MediaSwitch", () => {
  const { coordinator, signals, hostMessages } = createCoordinator();
  coordinator.handleControllerSignal({
    type: "hello",
    protocolVersion: REMOTE_DESKTOP_PROTOCOL_VERSION,
  });
  signals.length = 0;

  coordinator.handleControllerSignal({ type: "select-monitor", displayId: "0" });
  assert.deepEqual(signals, [{ type: "media-open", key: "a".repeat(64), salt: "b".repeat(16) }]);
  assert.deepEqual(hostMessages, [
    {
      type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
      rect: { x: 0, y: 0, width: 1920, height: 1080 },
      virtualRect: { x: -1280, y: 0, width: 3200, height: 1080 },
      key: "a".repeat(64),
      salt: "b".repeat(16),
    },
  ]);

  coordinator.handleControllerSignal({ type: "select-monitor", displayId: "1" });
  assert.deepEqual(hostMessages.at(-1), {
    type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaSwitch,
    rect: { x: -1280, y: 0, width: 1280, height: 1024 },
    virtualRect: { x: -1280, y: 0, width: 3200, height: 1080 },
  });
  assert.equal(signals.length, 1, "换屏不得重复 media-open");
});

test("select-monitor 未知显示器 / 无显示器：稳定错误码", () => {
  const unknown = createCoordinator();
  unknown.coordinator.handleControllerSignal({ type: "hello", protocolVersion: 2 });
  unknown.signals.length = 0;
  unknown.coordinator.handleControllerSignal({ type: "select-monitor", displayId: "missing" });
  assert.deepEqual(unknown.signals, [
    { type: "error", code: "internal", message: "unknown monitor" },
  ]);

  const empty = createCoordinator([]);
  empty.coordinator.handleControllerSignal({ type: "hello", protocolVersion: 2 });
  assert.deepEqual(empty.signals, [
    { type: "error", code: "unsupported", message: "local displays unavailable" },
  ]);
});

test("协议版本不匹配 → error{unsupported}", () => {
  const { coordinator, signals } = createCoordinator();
  coordinator.handleControllerSignal({ type: "hello", protocolVersion: 1 });
  assert.deepEqual(signals, [
    {
      type: "error",
      code: "unsupported",
      message: "protocol version 1 is not supported",
    },
  ]);
});

test("MediaStatus：冻结/解冻/起不来", () => {
  const { coordinator, signals } = createCoordinator();
  coordinator.handleMediaStatus({ frozen: true, reason: "locked" });
  coordinator.handleMediaStatus({ frozen: true, reason: "capture-error" });
  coordinator.handleMediaStatus({ frozen: false });
  coordinator.handleMediaStatus({ frozen: false, unavailable: "rd_media_start_failed" });
  assert.deepEqual(signals, [
    { type: "freeze", reason: "locked" },
    { type: "freeze", reason: "access-lost" },
    { type: "unfreeze" },
    { type: "error", code: "unsupported", message: "rd_media_start_failed" },
  ]);
});

test("会话关闭发 MediaStop；reset 后下一次 select 重新 MediaStart", () => {
  const { coordinator, signals, hostMessages } = createCoordinator();
  coordinator.handleControllerSignal({ type: "hello", protocolVersion: 2 });
  coordinator.handleControllerSignal({ type: "select-monitor", displayId: "0" });
  coordinator.handleSessionClosed();
  assert.equal(hostMessages.at(-1)?.type, RemoteDesktopHostMessageTypes.RemoteDesktopMediaStop);

  hostMessages.length = 0;
  signals.length = 0;
  coordinator.handleControllerSignal({ type: "select-monitor", displayId: "0" });
  assert.equal(
    hostMessages.at(-1)?.type,
    RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
    "会话关闭后同一协调器再次 select 必须重新开流",
  );
});
