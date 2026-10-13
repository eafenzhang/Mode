/**
 * 被控媒体面（P2）出站规划纯函数：coordinator 的解释执行只负责「按序发送」，载荷与顺序
 * 契约收敛在此，便于无 Electron 运行时的单测（tests/fixtures/remote-desktop-coordinator.ts）。
 * 只依赖消息常量与类型，不碰窗口/网络/时钟。
 */
import type { RemoteDesktopSignalMessage } from "@mode/shared/remote-desktop";
import {
  RemoteDesktopHostMessageTypes,
  type RemoteDesktopMediaRect,
  type RemoteDesktopMessageToHost,
} from "./hostMessages.js";

/** 一条待发出站：signal = 信令（经 SignalToHost 带出给对端），host = main→host 消息直发。 */
export type RemoteDesktopMediaOutbound =
  | { kind: "signal"; message: RemoteDesktopSignalMessage }
  | { kind: "host"; message: RemoteDesktopMessageToHost };

/**
 * 冻结原因归一：信令 freeze{reason} 的 union 是协议面（只认 locked|access-lost），而 helper 的
 * reason 是实现面错误串（capture-error / encode-error / access-lost / locked…），不能原样进协议——
 * 只有 locked 保真，其余一律按 access-lost 呈现（对端占位语义相同：画面不可用，恢复后自动 unfreeze）。
 */
export function normalizeMediaFreezeReason(reason: string | undefined): "locked" | "access-lost" {
  return reason === "locked" ? "locked" : "access-lost";
}

/**
 * MediaStatus（helper kind1）出站规划：frozen:true → freeze（reason 走归一）；
 * frozen:false → unfreeze（不带 reason——解冻没有理由维度，畸形输入也只出 unfreeze）。
 */
export function buildMediaStatusOutbound(
  frozen: boolean,
  reason?: string,
): RemoteDesktopMediaOutbound[] {
  if (frozen) {
    return [{ kind: "signal", message: { type: "freeze", reason: normalizeMediaFreezeReason(reason) } }];
  }
  return [{ kind: "signal", message: { type: "unfreeze" } }];
}

/**
 * start-capture 出站序列：仅 MediaStart（broker rd_media_start 拉起 helper 采集）。
 * media-open 信令**不在此下发**：同意时刻主控信令 WS 尚未 attach（ticket 随同意后的 POST
 * 响应才签发），会被被控 host 按「无会话连接」静默丢弃，主控永远拿不到 key/salt、媒体 WS
 * 永不建立（画面永不出根因）。改为首个 select-monitor 应答时经已 attach 的信令 WS 下发
 * （见 buildSelectMonitorOutbound）。key/salt 仍随 MediaStart 交给 helper 做逐帧加密。
 * rect/virtualRect（Task 14 fix round 1）：采集区域显示器物理矩形 + 虚拟屏 union 矩形，
 * host 侧据此把 renderer 的归一化输入坐标换算为 VIRTUALDESK 绝对坐标。
 */
export function buildStartCaptureOutbound(input: {
  rect: RemoteDesktopMediaRect;
  virtualRect: RemoteDesktopMediaRect;
  key: string;
  salt: string;
}): RemoteDesktopMediaOutbound[] {
  return [
    {
      kind: "host",
      message: {
        type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
        rect: input.rect,
        virtualRect: input.virtualRect,
        key: input.key,
        salt: input.salt,
      },
    },
  ];
}

/**
 * select-monitor 出站：rect 解析失败 → error{internal} 信令；成功 → [media-open（仅首个）]
 * + MediaSwitch（helper 换屏 + host 刷新输入换算基准——virtualRect 随换屏一并更新）。
 * mediaOpen 由 coordinator 仅在会话首个 select-monitor 传入：此时信令 WS 已 attach，
 * media-open 不会被丢弃；后续换屏只发 MediaSwitch（重复 media-open 会让主控重建媒体会话）。
 */
export function buildSelectMonitorOutbound(
  rects: { rect: RemoteDesktopMediaRect; virtualRect: RemoteDesktopMediaRect } | null,
  mediaOpen?: { key: string; salt: string },
): RemoteDesktopMediaOutbound[] {
  if (!rects) {
    return [
      { kind: "signal", message: { type: "error", code: "internal", message: "unknown monitor" } },
    ];
  }
  const outbounds: RemoteDesktopMediaOutbound[] = [];
  if (mediaOpen) {
    outbounds.push({
      kind: "signal",
      message: { type: "media-open", key: mediaOpen.key, salt: mediaOpen.salt },
    });
  }
  outbounds.push({
    kind: "host",
    message: {
      type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaSwitch,
      rect: rects.rect,
      virtualRect: rects.virtualRect,
    },
  });
  return outbounds;
}
