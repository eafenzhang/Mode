// LAN 远程桌面（Windows ↔ Windows）协议面：信令消息、输入事件与坐标/键盘纯函数。
// 实现与规则见 docs/specs/lan-remote-desktop-native-media.md（媒体面/输入传输）；产品规则仍见
// docs/specs/lan-remote-desktop.md。信令经被控端宿主转发（不解析内容），
// 输入事件走原生媒体面同一条 media WS 上行（JSON；pointer 移动采样允许丢失，原 control 语义
// 的离散事件不允许丢失）。
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
  // ---- 断线重连（P2-1）----
  // 主控 → 被控：媒体 WS 瞬断，请求重签一张媒体票（单次消费、30s、绑定同 clientId）；
  // 信令通道存活时可用。被控在会话 WS 上直回 media-ticket（不落信令转发面）。
  z.object({ type: z.literal("request-media-ticket") }).strict(),
  // 被控 → 主控：新的媒体票（renderer 用它替换 mediaUrl 的 mediaTicket 后重连媒体 WS）。
  z.object({ type: z.literal("media-ticket"), mediaTicket: z.string().trim().min(1) }).strict(),
]);
export type RemoteDesktopSignalMessage = z.infer<typeof remoteDesktopSignalMessageSchema>;

// ---- 输入事件 ----
export const remoteDesktopMouseButtonSchema = z.enum(["left", "right", "middle"]);
export type RemoteDesktopMouseButton = z.infer<typeof remoteDesktopMouseButtonSchema>;

// pointer 事件（媒体 WS 上行的移动采样）：允许丢失（传输未就绪/断开时由发送侧静默丢弃）。
export const remoteDesktopPointerEventSchema = z
  .object({
    t: z.literal("pm"),
    x: z.number().min(0).max(1),
    y: z.number().min(0).max(1),
  })
  .strict();
export type RemoteDesktopPointerEvent = z.infer<typeof remoteDesktopPointerEventSchema>;

// control 事件（媒体 WS 上行的离散事件）：不允许丢失（同一有序 TCP 链路内即时发送）。
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

// ---- 画质档位（性能优化方案 P1-3）----
// 主控在发起会话时选择，随会话请求下发（POST /api/remote-desktop/session?quality=）；
// 被控据此决定采集/编码参数（fps/maxWidth/初始码率），自适应码率在
// [minBitrateBps, bitrateBps] 两档之间调节（见 ui 的 adaptiveBitrate）。
// 默认 sharp = 与前版本硬编码参数一致（1920/60/8Mbps），不改默认行为的量级。
export const remoteDesktopQualitySchema = z.enum(["sharp", "balanced", "smooth"]);
export type RemoteDesktopQuality = z.infer<typeof remoteDesktopQualitySchema>;

export const DEFAULT_REMOTE_DESKTOP_QUALITY: RemoteDesktopQuality = "sharp";

export const REMOTE_DESKTOP_QUALITY_PRESETS: Record<
  RemoteDesktopQuality,
  {
    fps: number;
    maxWidth: number;
    bitrateBps: number;
    /** 自适应码率低档（拥塞时降到这一档，不再继续降）。 */
    minBitrateBps: number;
  }
> = {
  sharp: { fps: 60, maxWidth: 1920, bitrateBps: 8_000_000, minBitrateBps: 4_000_000 },
  balanced: { fps: 60, maxWidth: 1920, bitrateBps: 5_000_000, minBitrateBps: 2_500_000 },
  smooth: { fps: 30, maxWidth: 1600, bitrateBps: 3_000_000, minBitrateBps: 1_500_000 },
};

// 编码 codec 偏好（HEVC 选型，spec: lan-remote-desktop-native-media.md 2026-10-13）：
// 主控按本地 `VideoDecoder.isConfigSupported` 预检后经 `POST?codec=` 下发；
// 被控无硬件 HEVC 编码器时回退 h264——**解码配置串始终由关键帧码流自描述派生**，
// 偏好与实际不一致是安全的。缺省/旧主控 = h264（三向旧新兼容，无需版本门）。
export const remoteDesktopCodecSchema = z.enum(["h264", "hevc"]);
export type RemoteDesktopCodec = z.infer<typeof remoteDesktopCodecSchema>;

export const DEFAULT_REMOTE_DESKTOP_CODEC: RemoteDesktopCodec = "h264";

// ---- 编码能力缓存（P1-3 → 2026-10-15 增 software 态）----

/**
 * 被控端编码器面的观测值（spec: native-media「编码能力缓存」/「软件编码回退」）：
 * - `unknown`：尚无真实会话观测（不猜能力）；
 * - `available`：会话建流用的是硬件编码器 MFT；
 * - `software`：本机没有可用的硬件编码器，会话回退**系统软件编码器**继续出画面
 *   ——**不是失败**，但 CPU 占用与延迟更高，设置页据此给代价提示；
 * - `unavailable`：硬件与软件两路都拿不到编码器，会话起不来（根因提示）。
 */
export const remoteDesktopEncoderCapabilitySchema = z.enum([
  "unknown",
  "available",
  "software",
  "unavailable",
]);
export type RemoteDesktopEncoderCapability = z.infer<typeof remoteDesktopEncoderCapabilitySchema>;

// ---- 被控设置面（LR-2）----

/** 已授权设备（只给设置页展示；授权事实挂在 lan client 记录上）。 */
export interface RemoteDesktopAuthorizedClient {
  id: string;
  label: string;
}

/** 被控设置面快照：总开关 + 已授权设备 + 编码能力缓存。 */
export interface RemoteDesktopServerState {
  enabled: boolean;
  authorizedClients: RemoteDesktopAuthorizedClient[];
  encoderCapability: RemoteDesktopEncoderCapability;
}

/** 对端能力探测结果（旧对端缺能力键 = false）。 */
export interface RemoteDesktopPeerCapability {
  remoteDesktop: boolean;
  remoteDesktopNative: boolean;
  online: boolean;
  name?: string;
  /** 本次探测命中的端点类型（公网优先、LAN 回退）；离线时缺省。 */
  endpointKind?: "lan" | "public";
  endpointHost?: string;
}

// ---- 主控会话（LR-3）----

export type RemoteDesktopControllerState = "connecting" | "connected" | "closed";

/**
 * 主控会话事件（renderer ← Host）。
 *
 * `state` 是幂等的连接生命周期；`signal` 是**非幂等**的对端信令——只发给当前订阅者，
 * 不缓冲、不重放（断线重连由 renderer 的会话级重连策略处理）。
 */
export type RemoteDesktopSessionEvent =
  | {
      kind: "state";
      state: RemoteDesktopControllerState;
      error?: RemoteDesktopErrorCode;
      /** 传输断开（非 bye/本地结束）：renderer 可走会话级重连退避。 */
      retryable?: boolean;
    }
  | { kind: "signal"; message: RemoteDesktopSignalMessage };

/** 主控发起结果：成功时携带媒体面地址（契约 E）。 */
export type RemoteDesktopStartSessionResult =
  | { ok: true; mediaUrl?: string }
  | { error: RemoteDesktopErrorCode };

/** 被控同意请求（Host → renderer；60s 无响应按超时拒绝）。 */
export interface RemoteDesktopConsentRequest {
  clientId: string;
  label: string;
}

export interface RemoteDesktopConsentResponse {
  clientId: string;
  decision: "allow" | "deny";
  remember: boolean;
  /** 用户拒绝与超时同为 deny，但主控需要不同稳定错误码。 */
  reason?: "user" | "timeout";
}

/**
 * 被控端显示器快照（renderer 从 Tauri 壳读到后推给 Host）。
 *
 * 位置/尺寸都是**物理像素**（VIRTUALDESK 坐标域）；`displayId` 只在本会话内稳定，
 * 由 renderer 按枚举顺序生成。Host 不直接枚举显示器（D7：OS 能力留在壳）。
 */
export interface RemoteDesktopLocalDisplay {
  displayId: string;
  label: string;
  xPx: number;
  yPx: number;
  widthPx: number;
  heightPx: number;
  scaleFactor: number;
  primary: boolean;
}

// ---- 剪贴板同步（P1.1，spec 产品规则 7）----

/**
 * 文本上限（UTF-16 码元口径）。剪贴板内容千奇百怪，一条巨型文本会把媒体 WS 与中继的
 * 单帧预算撑爆（明文 JSON 同样占一帧），故超限**不下发**：同步是便利功能，
 * 不值得为它引入分片协议。
 */
export const REMOTE_DESKTOP_CLIPBOARD_MAX_CHARS = 128 * 1024;

/** 媒体 WS 的剪贴板 JSON：两个方向同形，方向由发送方决定。 */
export const remoteDesktopClipboardEventSchema = z
  .object({
    t: z.literal("clip"),
    text: z.string().min(1).max(REMOTE_DESKTOP_CLIPBOARD_MAX_CHARS),
  })
  .strict();
export type RemoteDesktopClipboardEvent = z.infer<typeof remoteDesktopClipboardEventSchema>;

/** 剪贴板同步开关（主控 renderer → 被控 host，经媒体 WS 上行；会话级，默认开）。 */
export const remoteDesktopClipboardModeSchema = z
  .object({ t: z.literal("clip-mode"), enabled: z.boolean() })
  .strict();
export type RemoteDesktopClipboardMode = z.infer<typeof remoteDesktopClipboardModeSchema>;

/**
 * 剪贴板双向同步的回环抑制状态机（纯逻辑；两端各持一个实例）。
 *
 * 为什么必须有它：收到对端文本 → 写入本端剪贴板 → 本端轮询立刻观测到「变化」→ 又发回对端
 * → 对端再写回来……不设门就是无限振荡。判据只有一条：**已经同步过的值再被观测到不算变化**，
 * 无论它是本端刚写进去的还是刚收到并应用的，都被登记为 lastSynced。
 *
 * 语义边界：
 * - 空文本不参与同步（「清空剪贴板」不是可表达的事件）；
 * - 超限文本两个方向都忽略：不下发，也不写入本端（不制造只有一侧存在的值）；
 * - 关闭不重置 lastSynced：重新打开时不会把关闭期间累积的值当作新变化弹回去。
 */
export interface RemoteDesktopClipboardSync {
  /** 本端剪贴板观测到文本：`"push"` = 应上行，`"ignore"` = 忽略。 */
  observeLocal(text: string | null): "push" | "ignore";
  /** 收到对端文本：`"apply"` = 应写入本端剪贴板，`"ignore"` = 忽略。 */
  observeRemote(text: string): "apply" | "ignore";
  setEnabled(enabled: boolean): void;
  isEnabled(): boolean;
}

export function createRemoteDesktopClipboardSync(
  enabledInitially = true,
): RemoteDesktopClipboardSync {
  let enabled = enabledInitially;
  let lastSynced: string | null = null;

  const usable = (text: string | null): text is string =>
    enabled &&
    typeof text === "string" &&
    text.length > 0 &&
    text.length <= REMOTE_DESKTOP_CLIPBOARD_MAX_CHARS;

  return {
    observeLocal(text) {
      if (!usable(text) || text === lastSynced) {
        return "ignore";
      }
      lastSynced = text;
      return "push";
    },
    observeRemote(text) {
      if (!usable(text) || text === lastSynced) {
        return "ignore";
      }
      lastSynced = text;
      return "apply";
    },
    setEnabled(next) {
      enabled = next;
    },
    isEnabled() {
      return enabled;
    },
  };
}
