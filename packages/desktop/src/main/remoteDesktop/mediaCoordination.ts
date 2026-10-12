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
 * start-capture 出站序列（spec 事件顺序 ②：先信令后启动——media-open 先把 key/salt 交给主控，
 * MediaStart 才拉起 helper 采集，主控解密就绪才收得住帧）。
 */
export function buildStartCaptureOutbound(input: {
  rect: RemoteDesktopMediaRect;
  key: string;
  salt: string;
}): RemoteDesktopMediaOutbound[] {
  return [
    { kind: "signal", message: { type: "media-open", key: input.key, salt: input.salt } },
    {
      kind: "host",
      message: {
        type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
        rect: input.rect,
        key: input.key,
        salt: input.salt,
      },
    },
  ];
}

/** select-monitor 出站：rect 解析失败 → error{internal} 信令；成功 → MediaSwitch 消息（helper 换屏）。 */
export function buildSelectMonitorOutbound(
  rect: RemoteDesktopMediaRect | null,
): RemoteDesktopMediaOutbound[] {
  if (!rect) {
    return [
      { kind: "signal", message: { type: "error", code: "internal", message: "unknown monitor" } },
    ];
  }
  return [
    { kind: "host", message: { type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaSwitch, rect } },
  ];
}
