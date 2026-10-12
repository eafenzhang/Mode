// LAN 远程桌面（Windows ↔ Windows）协议面：信令消息、输入事件与坐标/键盘纯函数。
// 实现与规则见 docs/specs/lan-remote-desktop.md；信令经被控端宿主转发（不解析内容），
// 输入事件走 WebRTC DataChannel 双通道（control 可靠 / pointer 可丢）。
import { z } from "zod";

// 协议版本 2（P3 信令 v2）：新增原生媒体面信令（media-open / freeze / unfreeze），
// 输入事件改走 media WS 上行。coordinator 的 hello 校验与主控 reducer 的 hello 发送
// 都引用本常量，两侧同构建天然同版本；旧版对端由 coordinator 回 unsupported 收口。
export const REMOTE_DESKTOP_PROTOCOL_VERSION = 2;

// ---- 显示器描述（被控端 → 主控端：供选择与坐标换算参照；物理像素口径）----
export const remoteDesktopMonitorSchema = z
  .object({
    displayId: z.string().trim().min(1),
    label: z.string(),
    widthPx: z.number().int().positive(),
    heightPx: z.number().int().positive(),
    scaleFactor: z.number().positive(),
    primary: z.boolean(),
  })
  .strict();
export type RemoteDesktopMonitor = z.infer<typeof remoteDesktopMonitorSchema>;

// ---- 信令消息 ----
export const remoteDesktopErrorCodeSchema = z.enum([
  "consent_denied",
  "consent_timeout",
  "disabled",
  "busy",
  "unsupported",
  "unauthorized",
  "stale_ticket",
  // 本机被控中发起主控（spec 产品规则 9：被控中不可发起，断开被控后重试）。
  "locally_controlled",
  "internal",
]);
export type RemoteDesktopErrorCode = z.infer<typeof remoteDesktopErrorCodeSchema>;

export const remoteDesktopSignalMessageSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("hello"), protocolVersion: z.number().int().positive() }).strict(),
  z
    .object({ type: z.literal("monitors"), monitors: z.array(remoteDesktopMonitorSchema).min(1) })
    .strict(),
  z.object({ type: z.literal("select-monitor"), displayId: z.string().trim().min(1) }).strict(),
  z.object({ type: z.literal("offer"), sdp: z.string().min(1) }).strict(),
  z.object({ type: z.literal("answer"), sdp: z.string().min(1) }).strict(),
  z
    .object({
      type: z.literal("ice"),
      candidate: z.string().min(1),
      sdpMid: z.string().nullable(),
      sdpMLineIndex: z.number().int().nullable(),
    })
    .strict(),
  // ---- P2 原生媒体面（契约 D，被控 → 主控）：media-open 携带加媒体面的密钥/盐，
  // freeze/unfreeze 表示被控端画面冻结状态（锁定/失去访问权限时冻结，恢复后解冻）。 ----
  z
    .object({
      type: z.literal("media-open"),
      key: z.string().regex(/^[0-9a-f]{64}$/u),
      salt: z.string().regex(/^[0-9a-f]{16}$/u),
    })
    .strict(),
  z.object({ type: z.literal("freeze"), reason: z.enum(["locked", "access-lost"]) }).strict(),
  z.object({ type: z.literal("unfreeze") }).strict(),
  z
    .object({
      type: z.literal("bye"),
      reason: z.enum(["user-end", "remote-end", "error"]).optional(),
    })
    .strict(),
  z
    .object({
      type: z.literal("error"),
      code: remoteDesktopErrorCodeSchema,
      message: z.string().optional(),
    })
    .strict(),
]);
export type RemoteDesktopSignalMessage = z.infer<typeof remoteDesktopSignalMessageSchema>;

// ---- 输入事件 ----
export const remoteDesktopMouseButtonSchema = z.enum(["left", "right", "middle"]);
export type RemoteDesktopMouseButton = z.infer<typeof remoteDesktopMouseButtonSchema>;

// pointer 通道（unordered + maxRetransmits:0）：移动采样允许丢失。
export const remoteDesktopPointerEventSchema = z
  .object({
    t: z.literal("pm"),
    x: z.number().min(0).max(1),
    y: z.number().min(0).max(1),
  })
  .strict();
export type RemoteDesktopPointerEvent = z.infer<typeof remoteDesktopPointerEventSchema>;

// control 通道（ordered + 可靠）：离散事件不允许丢失。
export const remoteDesktopControlEventSchema = z.discriminatedUnion("t", [
  z
    .object({
      t: z.literal("md"),
      button: remoteDesktopMouseButtonSchema,
      x: z.number().min(0).max(1),
      y: z.number().min(0).max(1),
    })
    .strict(),
  z
    .object({
      t: z.literal("mu"),
      button: remoteDesktopMouseButtonSchema,
      x: z.number().min(0).max(1),
      y: z.number().min(0).max(1),
    })
    .strict(),
  z
    .object({
      t: z.literal("mw"),
      direction: z.enum(["up", "down", "left", "right"]),
      delta: z.number(),
      x: z.number().min(0).max(1),
      y: z.number().min(0).max(1),
    })
    .strict(),
  z.object({ t: z.literal("kd"), vk: z.number().int().min(0).max(255) }).strict(),
  z.object({ t: z.literal("ku"), vk: z.number().int().min(0).max(255) }).strict(),
  z.object({ t: z.literal("tx"), text: z.string().min(1) }).strict(),
]);
export type RemoteDesktopControlEvent = z.infer<typeof remoteDesktopControlEventSchema>;

// ---- 纯函数（renderer 采集侧 / 被控端注入侧共用；纯函数单测见 test/remote-desktop.test.ts）----

// object-fit: contain 下，指针在视频元素内的位置 → 画面归一化坐标（x/y ∈ [0,1]）。
// letterbox 区域内的点 clamp 到边缘并标记 inside:false，保证边缘拖拽仍可用。
export function computeNormalizedPointFromVideoEvent(
  videoRect: { left: number; top: number; width: number; height: number },
  intrinsic: { width: number; height: number },
  client: { x: number; y: number },
): { x: number; y: number; inside: boolean } {
  const rectW = videoRect.width;
  const rectH = videoRect.height;
  const iw = intrinsic.width;
  const ih = intrinsic.height;
  if (!(rectW > 0 && rectH > 0 && iw > 0 && ih > 0)) {
    return { x: 0, y: 0, inside: false };
  }
  const scale = Math.min(rectW / iw, rectH / ih);
  const contentW = iw * scale;
  const contentH = ih * scale;
  const offsetX = videoRect.left + (rectW - contentW) / 2;
  const offsetY = videoRect.top + (rectH - contentH) / 2;
  const nx = (client.x - offsetX) / contentW;
  const ny = (client.y - offsetY) / contentH;
  const inside = nx >= 0 && nx <= 1 && ny >= 0 && ny <= 1;
  const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
  return { x: clamp01(nx), y: clamp01(ny), inside };
}

const MODIFIER_VK: Record<string, number> = {
  Shift: 16,
  Control: 17,
  Alt: 18,
  Meta: 91,
};

// 键盘决策规则（spike 教训：文本与 VK 不能混发，否则单键会打两遍）：
// - 修饰键（Shift/Ctrl/Alt/Meta）与 Ctrl/Alt/Meta 组合键、不可打印键 → VK（Chromium keyCode
//   即 Windows VK 口径，含字母/数字/方向/功能键）；
// - 纯可打印字符 → Unicode 文本（对端以 KEYEVENTF_UNICODE 注入，天然覆盖输入法组合结果与
//   大小写，不依赖对端键位布局）。
export function describeKeyAction(ev: {
  key: string;
  keyCode: number;
  ctrlKey?: boolean;
  altKey?: boolean;
  metaKey?: boolean;
}): { kind: "text"; text: string } | { kind: "vk"; vk: number } | { kind: "none" } {
  const modifierVk = MODIFIER_VK[ev.key];
  if (modifierVk !== undefined) {
    return { kind: "vk", vk: modifierVk };
  }
  const softModifiers = Boolean(ev.ctrlKey) || Boolean(ev.altKey) || Boolean(ev.metaKey);
  if (!softModifiers && ev.key.length === 1) {
    return { kind: "text", text: ev.key };
  }
  if (ev.keyCode > 0 && ev.keyCode <= 255) {
    return { kind: "vk", vk: ev.keyCode };
  }
  return { kind: "none" };
}

// 显示器内归一化坐标 → 虚拟屏绝对坐标（0..65535，MOUSEEVENTF_ABSOLUTE|VIRTUALDESK 口径）。
// 覆盖多显示器负原点；边界取 (widthPx - 1) 使 1.0 精确落在最右/最下像素。
export function mapMonitorNormalizedToVirtualAbsolute(
  p: { x: number; y: number },
  monitor: { xPx: number; yPx: number; widthPx: number; heightPx: number },
  virtual: { xPx: number; yPx: number; widthPx: number; heightPx: number },
): { x: number; y: number } {
  if (
    !(monitor.widthPx > 0 && monitor.heightPx > 0 && virtual.widthPx > 0 && virtual.heightPx > 0) ||
    !Number.isFinite(p.x) ||
    !Number.isFinite(p.y)
  ) {
    return { x: 0, y: 0 };
  }
  const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
  const cx = clamp01(p.x);
  const cy = clamp01(p.y);
  const absX = monitor.xPx + cx * (monitor.widthPx - 1);
  const absY = monitor.yPx + cy * (monitor.heightPx - 1);
  const nx = (absX - virtual.xPx) / Math.max(1, virtual.widthPx - 1);
  const ny = (absY - virtual.yPx) / Math.max(1, virtual.heightPx - 1);
  const toU16 = (v: number) => Math.min(65535, Math.max(0, Math.round(v * 65535)));
  return { x: toU16(nx), y: toU16(ny) };
}
