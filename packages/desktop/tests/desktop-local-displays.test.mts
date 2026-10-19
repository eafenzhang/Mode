/**
 * LR-3：Tauri 显示器 → 被控协议快照的映射，以及 renderer → Host 的推送。
 * 纯函数 + 假 `window.__TAURI__`；不需要真窗口。
 */
import assert from "node:assert/strict";
import test from "node:test";

import type { IServiceAccessor } from "@mode/services";
import type { RemoteDesktopLocalDisplay } from "@mode/shared/remote-desktop";

import {
  mapTauriMonitors,
  syncLocalDisplaysToHost,
} from "../src/renderer/src/desktopLocalDisplays.js";

test("mapTauriMonitors：物理尺寸/位置、primary 判定与 displayId 顺序", () => {
  const mapped = mapTauriMonitors([
    {
      name: "Primary",
      size: { width: 1920, height: 1080 },
      position: { x: 0, y: 0 },
      scaleFactor: 1,
    },
    {
      name: "",
      size: { width: 1280, height: 1024 },
      position: { x: -1280, y: 0 },
      scaleFactor: 1.25,
    },
  ]);
  assert.deepEqual(mapped, [
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
      label: "Display 2",
      xPx: -1280,
      yPx: 0,
      widthPx: 1280,
      heightPx: 1024,
      scaleFactor: 1.25,
      primary: false,
    },
  ]);
});

test("mapTauriMonitors：没有原点显示器时第一个兜底为 primary；非法尺寸丢弃", () => {
  const mapped = mapTauriMonitors([
    { size: { width: 800, height: 600 }, position: { x: 100, y: 100 } },
    { size: { width: 0, height: 600 }, position: { x: 0, y: 0 } },
  ]);
  assert.equal(mapped.length, 1);
  assert.equal(mapped[0]?.primary, true);
  assert.equal(mapped[0]?.scaleFactor, 1);
});

test("syncLocalDisplaysToHost：非 Tauri 静默跳过；Tauri 下推给 Host 服务", async () => {
  const captured: RemoteDesktopLocalDisplay[][] = [];
  const services = {
    remoteDesktopService: {
      setLocalDisplays: async (params: { displays: RemoteDesktopLocalDisplay[] }) => {
        captured.push(params.displays);
      },
    },
  } as unknown as IServiceAccessor;

  const scope = globalThis as unknown as { __TAURI__?: unknown };
  const hadTauri = "__TAURI__" in scope;
  const previous = scope.__TAURI__;
  try {
    delete scope.__TAURI__;
    await syncLocalDisplaysToHost(services);
    assert.deepEqual(captured, [], "非 Tauri 不得推送");

    scope.__TAURI__ = {
      window: {
        availableMonitors: async () => [
          { name: "Only", size: { width: 1920, height: 1080 }, position: { x: 0, y: 0 } },
        ],
      },
    };
    await syncLocalDisplaysToHost(services);
    assert.deepEqual(captured, [
      [
        {
          displayId: "0",
          label: "Only",
          xPx: 0,
          yPx: 0,
          widthPx: 1920,
          heightPx: 1080,
          scaleFactor: 1,
          primary: true,
        },
      ],
    ]);
  } finally {
    if (hadTauri) {
      scope.__TAURI__ = previous;
    } else {
      delete scope.__TAURI__;
    }
  }
});
