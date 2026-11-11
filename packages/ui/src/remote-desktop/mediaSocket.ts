import type { RemoteDesktopMediaSocket } from "@mode/shared/remote-desktop";

/** WebSocket OPEN；契约不依赖 DOM 常量，桌面与浏览器共用同一判定。 */
export const MEDIA_SOCKET_OPEN = 1;

/**
 * 缺省直连 DOM WebSocket；浏览器 v3 注入 E2EE 隧道适配器（无本机回环地址可连）。
 * 契约的唯一所有者在 `@mode/shared/remote-desktop`。
 */
export function openMediaSocket(
  factory: ((url: string) => RemoteDesktopMediaSocket) | undefined,
  url: string,
): RemoteDesktopMediaSocket {
  return factory?.(url) ?? (new WebSocket(url) as unknown as RemoteDesktopMediaSocket);
}
