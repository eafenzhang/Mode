/**
 * peer-session-open 回帖（host → main）到 main 侧 pending resolve 载荷的映射。
 *
 * 从 hostBridge 抽出为纯函数：契约 E 的 mediaUrl 必须透传到 main 出口——它是 renderer 直连
 * media WS 的唯一地址来源，settle 处丢字段会让主控媒体面静默连不上（Task 10 消费端拿恒
 * undefined）。tests/fixtures/remote-desktop-coordinator.ts 对此有回归断言。
 * 本模块无 Electron 依赖，可在单测运行时直接导入。
 */
import {
  RemoteDesktopHostMessageTypes,
  type RemoteDesktopMessageFromHost,
} from "./hostMessages.js";

export type RemoteDesktopPeerSessionOpenOutcome =
  | { ok: true; wsUrl: string; mediaUrl?: string }
  | { ok: false; errorCode?: string; error: string };

export function buildPeerSessionOpenOutcome(
  message: Extract<
    RemoteDesktopMessageFromHost,
    { type: typeof RemoteDesktopHostMessageTypes.RemoteDesktopPeerSessionOpenResult }
  >,
): RemoteDesktopPeerSessionOpenOutcome {
  return message.ok && message.wsUrl
    ? { ok: true, wsUrl: message.wsUrl, ...(message.mediaUrl ? { mediaUrl: message.mediaUrl } : {}) }
    : {
        ok: false,
        ...(message.errorCode ? { errorCode: message.errorCode } : {}),
        error: message.error ?? "对端会话请求失败",
      };
}
