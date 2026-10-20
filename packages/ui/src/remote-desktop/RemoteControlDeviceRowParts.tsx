import type { ReactNode } from "react";
import type { McpServerStatus } from "@mode/shared";
import { cn } from "@/components/lib/utils.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import type { RemoteDesktopDeviceStatusTone } from "@/remote-desktop/deviceAvailability.js";
import type { DeviceViewMode } from "@/remote-desktop/useDeviceViewMode.js";

const STATUS_TONES: Record<RemoteDesktopDeviceStatusTone, { pill: string; dot: string }> = {
  ready: { pill: "bg-success text-success-foreground", dot: "bg-success-foreground" },
  attention: { pill: "bg-warning/15 text-warning", dot: "bg-warning" },
  muted: { pill: "bg-surface text-foreground-subtle", dot: "bg-foreground-subtlest" },
};

export function StatusPill({
  labelId,
  tone,
  pulsing = false,
}: {
  labelId: string;
  tone: RemoteDesktopDeviceStatusTone;
  pulsing?: boolean;
}) {
  const { intl } = useModeIntl();
  const style = STATUS_TONES[tone];
  return (
    <span
      className={cn(
        "inline-flex h-5 shrink-0 items-center gap-1.5 rounded-full px-2 text-ui-xs font-medium",
        style.pill,
      )}
    >
      <span className={cn("size-1.5 rounded-full", style.dot, pulsing && "animate-pulse")} />
      {intl.formatMessage({ id: labelId })}
    </span>
  );
}

function KindBadge({ labelId }: { labelId: string }) {
  const { intl } = useModeIntl();
  return (
    <span className="shrink-0 rounded bg-surface px-1.5 py-0.5 text-ui-xs text-foreground-subtle">
      {intl.formatMessage({ id: labelId })}
    </span>
  );
}

export function RowShell({
  testId,
  children,
  editor,
  presentation = "list",
}: {
  testId: string;
  children: ReactNode;
  editor?: ReactNode;
  presentation?: DeviceViewMode;
}) {
  return (
    <li
      data-testid={testId}
      data-view={presentation}
      className={
        presentation === "grid"
          ? "flex h-full flex-col gap-3 rounded-xl border border-border bg-card p-3 transition-colors hover:border-input-border-hover"
          : "flex flex-col gap-2 px-3 py-2.5 transition-colors hover:bg-surface-hover"
      }
    >
      <div
        className={
          presentation === "grid"
            ? "flex flex-1 flex-col justify-between gap-3"
            : "flex flex-wrap items-center justify-between gap-3"
        }
      >
        {children}
      </div>
      {editor}
    </li>
  );
}

export function DeviceIdentity({
  pill,
  name,
  kindLabelId,
  address,
  detail,
  addressTestId,
}: {
  pill: ReactNode;
  name: string;
  kindLabelId: string;
  address: string;
  detail?: string;
  addressTestId?: string;
}) {
  return (
    <div className="min-w-0 flex-1">
      <div className="flex flex-wrap items-center gap-2">
        {pill}
        <span className="min-w-0 truncate text-ui-base font-semibold text-foreground">{name}</span>
        <KindBadge labelId={kindLabelId} />
      </div>
      <p className="mt-0.5 truncate text-ui-xs text-foreground-subtlest">
        <span className="font-mono" data-testid={addressTestId}>
          {address}
        </span>
        {detail ? <span className="text-foreground-subtle"> · {detail}</span> : null}
      </p>
    </div>
  );
}

export function modeAgentStatus(status: McpServerStatus | undefined): {
  labelId: string;
  tone: RemoteDesktopDeviceStatusTone;
  pulsing: boolean;
} {
  switch (status) {
    case "connected":
      return {
        labelId: "remoteDesktop.modeAgents.statusConnected",
        tone: "ready",
        pulsing: false,
      };
    case "connecting":
      return {
        labelId: "remoteDesktop.modeAgents.statusConnecting",
        tone: "muted",
        pulsing: true,
      };
    case "error":
    case "disconnected":
      return {
        labelId: "remoteDesktop.modeAgents.statusOffline",
        tone: "attention",
        pulsing: false,
      };
    default:
      return {
        labelId: "remoteDesktop.modeAgents.statusUnknown",
        tone: "muted",
        pulsing: false,
      };
  }
}
