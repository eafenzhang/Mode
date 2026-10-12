import assert from "node:assert/strict";
import test from "node:test";
import {
  REMOTE_DESKTOP_PROTOCOL_VERSION,
  computeNormalizedPointFromVideoEvent,
  describeKeyAction,
  mapMonitorNormalizedToVirtualAbsolute,
  remoteDesktopControlEventSchema,
  remoteDesktopPointerEventSchema,
  remoteDesktopSignalMessageSchema,
} from "../src/remote-desktop.js";
import { serverRemoteInfoSchema } from "../src/server-remote.js";

// spec: docs/specs/lan-remote-desktop.md
// 协议面是主控/被控两侧与将来 Rust 实现的共同契约：先钉死形状与纯函数边界，再谈接线。

test("协议版本为 2（原生媒体信令面），hello 必须携带版本号", () => {
  assert.equal(REMOTE_DESKTOP_PROTOCOL_VERSION, 2);
  assert.equal(
    remoteDesktopSignalMessageSchema.safeParse({ type: "hello", protocolVersion: 2 }).success,
    true,
  );
  assert.equal(remoteDesktopSignalMessageSchema.safeParse({ type: "hello" }).success, false);
});

test("信令消息：monitors 至少一项、未知类型被拒、error 码白名单", () => {
  assert.equal(
    remoteDesktopSignalMessageSchema.safeParse({ type: "monitors", monitors: [] }).success,
    false,
  );
  assert.equal(
    remoteDesktopSignalMessageSchema.safeParse({
      type: "monitors",
      monitors: [
        { displayId: "d1", label: "显示器 1", widthPx: 1920, heightPx: 1080, scaleFactor: 1, primary: true },
      ],
    }).success,
    true,
  );
  assert.equal(remoteDesktopSignalMessageSchema.safeParse({ type: "nope" }).success, false);
  assert.equal(
    remoteDesktopSignalMessageSchema.safeParse({
      type: "ice",
      candidate: "candidate:1 1 udp 2122194687 192.168.1.4 56050 typ host",
      sdpMid: "0",
      sdpMLineIndex: 0,
    }).success,
    true,
  );
  assert.equal(
    remoteDesktopSignalMessageSchema.safeParse({ type: "error", code: "consent_denied" }).success,
    true,
  );
  assert.equal(
    remoteDesktopSignalMessageSchema.safeParse({ type: "error", code: "wat" }).success,
    false,
  );
});

test("原生媒体信令：media-open / freeze / unfreeze 可解析", () => {
  const ok = remoteDesktopSignalMessageSchema.safeParse({
    type: "media-open",
    key: "a".repeat(64),
    salt: "b".repeat(16),
  });
  assert.equal(ok.success, true);
  const frozen = remoteDesktopSignalMessageSchema.safeParse({ type: "freeze", reason: "locked" });
  assert.equal(frozen.success, true);
  const thaw = remoteDesktopSignalMessageSchema.safeParse({ type: "unfreeze" });
  assert.equal(thaw.success, true);
  // 严格模式：未知字段仍拒绝
  const bad = remoteDesktopSignalMessageSchema.safeParse({ type: "unfreeze", extra: 1 });
  assert.equal(bad.success, false);
});

test("输入事件：pointer 越界被拒、control 的 vk 范围与文本非空", () => {
  assert.equal(remoteDesktopPointerEventSchema.safeParse({ t: "pm", x: 0.5, y: 1 }).success, true);
  assert.equal(remoteDesktopPointerEventSchema.safeParse({ t: "pm", x: 1.2, y: 0 }).success, false);
  assert.equal(remoteDesktopControlEventSchema.safeParse({ t: "kd", vk: 65 }).success, true);
  assert.equal(remoteDesktopControlEventSchema.safeParse({ t: "kd", vk: 300 }).success, false);
  assert.equal(remoteDesktopControlEventSchema.safeParse({ t: "tx", text: "" }).success, false);
  assert.equal(
    remoteDesktopControlEventSchema.safeParse({ t: "md", button: "middle", x: 0.1, y: 0.1 }).success,
    true,
  );
  assert.equal(
    remoteDesktopControlEventSchema.safeParse({ t: "mw", direction: "up", delta: -120, x: 0.5, y: 0.5 })
      .success,
    true,
  );
});

test("video letterbox 映射：中心点、letterbox 内点（clamp+outside）、内容上边缘含点、退化输入", () => {
  const rect = { left: 100, top: 50, width: 800, height: 600 };
  const intrinsic = { width: 1920, height: 1080 };
  assert.deepEqual(computeNormalizedPointFromVideoEvent(rect, intrinsic, { x: 500, y: 350 }), {
    x: 0.5,
    y: 0.5,
    inside: true,
  });
  const topBar = computeNormalizedPointFromVideoEvent(rect, intrinsic, { x: 500, y: 60 });
  assert.equal(topBar.inside, false);
  assert.equal(topBar.y, 0);
  assert.equal(topBar.x, 0.5);
  const topEdge = computeNormalizedPointFromVideoEvent(rect, intrinsic, { x: 500, y: 125 });
  assert.equal(topEdge.inside, true);
  assert.equal(topEdge.y, 0);
  assert.deepEqual(computeNormalizedPointFromVideoEvent(rect, { width: 0, height: 0 }, { x: 500, y: 350 }), {
    x: 0,
    y: 0,
    inside: false,
  });
});

test("键盘决策：可打印字符走文本、修饰键/组合键/导航键走 VK、未知键 none", () => {
  assert.deepEqual(describeKeyAction({ key: "a", keyCode: 65 }), { kind: "text", text: "a" });
  assert.deepEqual(describeKeyAction({ key: "A", keyCode: 65 }), { kind: "text", text: "A" });
  assert.deepEqual(describeKeyAction({ key: "C", keyCode: 67, ctrlKey: true }), {
    kind: "vk",
    vk: 67,
  });
  assert.deepEqual(describeKeyAction({ key: "1", keyCode: 49, altKey: true }), {
    kind: "vk",
    vk: 49,
  });
  assert.deepEqual(describeKeyAction({ key: "Shift", keyCode: 16 }), { kind: "vk", vk: 16 });
  assert.deepEqual(describeKeyAction({ key: "Control", keyCode: 17 }), { kind: "vk", vk: 17 });
  assert.deepEqual(describeKeyAction({ key: "ArrowLeft", keyCode: 37 }), { kind: "vk", vk: 37 });
  assert.deepEqual(describeKeyAction({ key: "Unidentified", keyCode: 0 }), { kind: "none" });
});

test("虚拟屏映射：单屏角点精确、邻屏起点、clamp 与非法输入", () => {
  const monitor = { xPx: 0, yPx: 0, widthPx: 1920, heightPx: 1080 };
  const virtualSingle = { xPx: 0, yPx: 0, widthPx: 1920, heightPx: 1080 };
  assert.deepEqual(mapMonitorNormalizedToVirtualAbsolute({ x: 0, y: 0 }, monitor, virtualSingle), {
    x: 0,
    y: 0,
  });
  assert.deepEqual(mapMonitorNormalizedToVirtualAbsolute({ x: 1, y: 1 }, monitor, virtualSingle), {
    x: 65535,
    y: 65535,
  });
  const secondMonitorStart = mapMonitorNormalizedToVirtualAbsolute(
    { x: 0, y: 0 },
    { xPx: 1920, yPx: 0, widthPx: 1920, heightPx: 1080 },
    { xPx: 0, yPx: 0, widthPx: 3840, heightPx: 1080 },
  );
  // 1920 / 3839 * 65535 = 32776.06 → 32776
  assert.equal(secondMonitorStart.x, 32776);
  assert.equal(secondMonitorStart.y, 0);
  assert.deepEqual(
    mapMonitorNormalizedToVirtualAbsolute({ x: 1.5, y: -0.5 }, monitor, virtualSingle),
    { x: 65535, y: 0 },
  );
  assert.deepEqual(
    mapMonitorNormalizedToVirtualAbsolute(
      { x: 0.5, y: 0.5 },
      { xPx: 0, yPx: 0, widthPx: 0, heightPx: 0 },
      virtualSingle,
    ),
    { x: 0, y: 0 },
  );
});

test("server-info 能力位：remoteDesktop 缺键兼容（旧对端）与真值解析", () => {
  const base = {
    serverId: "s1",
    version: "1.0.0",
    protocolVersion: 1,
    authRequired: true,
    workspaces: [],
    capabilities: { desktopContinuous: true, websocketRpc: true },
  };
  const old = serverRemoteInfoSchema.safeParse(base);
  assert.equal(old.success, true);
  if (old.success) {
    assert.equal(old.data.capabilities.remoteDesktop, undefined);
  }
  const withCap = serverRemoteInfoSchema.safeParse({
    ...base,
    capabilities: { desktopContinuous: true, websocketRpc: true, remoteDesktop: true },
  });
  assert.equal(withCap.success, true);
  if (withCap.success) {
    assert.equal(withCap.data.capabilities.remoteDesktop, true);
  }
});
