/**
 * 「远程桌面」设备页 · 已连接会话列表（spec: docs/specs/remote-desktop-system-rdp.md 事件顺序 ③④）。
 *
 * 会话发生在独立 mstsc 窗口中，应用零参与；本列表只是主控 main 会话表（`rdGetSessions`）的
 * 只读投影 + 「断开」（`rdDisconnect`）入口。单一来源：会话记录只从 main 读，列表不维护副本。
 * 空列表不渲染（避免设备页出现常驻空块；列表出现本身即「有会话在跑」的信号）。
 */
import { MonitorSmartphone, X } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { formatDateTime } from "@/settings/automationFormat.js";
import type { RemoteDesktopSessionRecord } from "@/remote-desktop/platformTypes.js";

export function RemoteDesktopSessionList({
  sessions,
  busy,
  onDisconnect,
}: {
  sessions: RemoteDesktopSessionRecord[];
  busy: boolean;
  onDisconnect: (sessionId: string) => void;
}) {
  const { intl } = useModeIntl();
  if (sessions.length === 0) {
    return null;
  }
  return (
    <section
      className="rounded-lg border border-border bg-card px-4 py-3"
      data-testid="remote-desktop-sessions"
      aria-label={intl.formatMessage({ id: "remoteDesktop.sessions.title" })}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="text-ui-base text-foreground">
          {intl.formatMessage({ id: "remoteDesktop.sessions.title" })}
        </span>
        <span className="text-ui-xs text-foreground-subtle">
          {intl.formatMessage(
            { id: "remoteDesktop.sessions.count" },
            { count: String(sessions.length) },
          )}
        </span>
      </div>
      <ul className="mt-3 space-y-2">
        {sessions.map((session) => (
          <li
            key={session.sessionId}
            data-testid="remote-desktop-session-row"
            className="flex items-start justify-between gap-3 rounded-lg border border-border bg-surface px-3 py-2"
          >
            <span className="flex min-w-0 flex-1 items-start gap-2">
              <MonitorSmartphone className="mt-0.5 size-4 shrink-0 text-foreground-subtle" />
              <span className="min-w-0">
                <span className="block truncate text-ui-base text-foreground">{session.name}</span>
                <span className="block truncate font-mono text-ui-xs text-foreground-subtle">
                  {session.address}
                </span>
                <span className="block text-ui-xs text-foreground-subtlest">
                  {intl.formatMessage(
                    { id: "remoteDesktop.sessions.startedAt" },
                    { time: formatDateTime(session.startedAt) },
                  )}
                </span>
              </span>
            </span>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="shrink-0"
              disabled={busy}
              aria-label={intl.formatMessage(
                { id: "remoteDesktop.sessions.disconnectFor" },
                { name: session.name },
              )}
              onClick={() => onDisconnect(session.sessionId)}
            >
              <X className="size-3.5" />
              {intl.formatMessage({ id: "remoteDesktop.sessions.disconnect" })}
            </Button>
          </li>
        ))}
      </ul>
      <p className="mt-2 text-ui-xs text-foreground-subtlest">
        {intl.formatMessage({ id: "remoteDesktop.sessions.hint" })}
      </p>
    </section>
  );
}
