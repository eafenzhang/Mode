import { useEffect, useMemo, useRef, useState } from "react";
import { createWebRemoteDesktopSession, type WebRemoteDesktopSession } from "@mode/client";
import type { WebControlLink } from "@mode/remote-core/web";
import {
  DEFAULT_REMOTE_DESKTOP_QUALITY,
  type RemoteDesktopCodec,
  type RemoteDesktopQuality,
  type RemoteDesktopSessionEvent,
  type RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";
import { ModeIntlProvider, RemoteDesktopSession } from "@mode/ui";

/**
 * `/v3` 的远程桌面会话面：把 `RemoteDesktopSession` 接到 v3 E2EE 隧道。
 *
 * 只提供会话所需的平台子集；媒体 socket 由同一 adapter 的 `openMediaSocket` 注入，
 * 现有解码/输入/统计/重连逻辑全部复用，不复制。
 */
export function WebV3RemoteDesktop({
  link,
  hostDeviceId,
  deviceName,
  onExit,
}: {
  link: WebControlLink;
  hostDeviceId: string;
  deviceName: string;
  onExit: () => void;
}) {
  const [quality, setQuality] = useState<RemoteDesktopQuality>(DEFAULT_REMOTE_DESKTOP_QUALITY);
  const sessionRef = useRef<WebRemoteDesktopSession | null>(null);
  const listenersRef = useRef(new Set<(event: RemoteDesktopSessionEvent) => void>());

  const platform = useMemo(
    () => ({
      rdStartSession: async (serverId: string, nextQuality?: RemoteDesktopQuality, codec?: RemoteDesktopCodec) => {
        sessionRef.current?.dispose();
        const session = createWebRemoteDesktopSession({
          link,
          hostDeviceId: serverId,
          ...(nextQuality ? { quality: nextQuality } : {}),
          ...(codec ? { codec } : {}),
          onEvent: (event) => {
            for (const listener of listenersRef.current) listener(event);
          },
        });
        sessionRef.current = session;
        return await session.start();
      },
      rdSendSignal: (message: RemoteDesktopSignalMessage) => {
        sessionRef.current?.sendSignal(message);
      },
      rdEndSession: async (reason?: "user-end" | "error") => {
        await sessionRef.current?.end(reason);
      },
      rdReadClipboardText: () => sessionRef.current?.readClipboardText() ?? Promise.resolve(""),
      onRemoteDesktopSessionEvent: (handler: (event: RemoteDesktopSessionEvent) => void) => {
        listenersRef.current.add(handler);
        return () => {
          listenersRef.current.delete(handler);
        };
      },
    }),
    [link],
  );

  const openMediaSocket = useMemo(
    () => (url: string) => {
      const session = sessionRef.current;
      if (!session) throw new Error("远程桌面会话尚未建立");
      return session.openMediaSocket(url);
    },
    [],
  );

  useEffect(
    () => () => {
      const session = sessionRef.current;
      sessionRef.current = null;
      if (session) void session.dispose();
    },
    [],
  );

  return (
    <ModeIntlProvider>
      <div className="h-screen w-screen overflow-hidden bg-background text-foreground">
        <RemoteDesktopSession
          serverId={hostDeviceId}
          deviceName={deviceName}
          quality={quality}
          onQualityChange={setQuality}
          onExit={onExit}
          platform={platform}
          openMediaSocket={openMediaSocket}
        />
      </div>
    </ModeIntlProvider>
  );
}
