import { useState } from "react";
import { Check, LoaderIcon, MessageSquarePlus, Pencil, RefreshCw, Trash2 } from "lucide-react";
import { Button } from "@/ui-foundation/adapters/button.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { cn } from "@/components/lib/utils.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import type { ModeAgentServerRecord } from "@/remote-desktop/modeAgentPairing.js";
import type { DeviceViewMode } from "@/remote-desktop/useDeviceViewMode.js";
import {
  DeviceIdentity,
  RowShell,
  StatusPill,
  modeAgentStatus,
} from "@/remote-desktop/RemoteControlDeviceRowParts.js";

export function RemoteControlModeAgentRow({
  record,
  presentation,
  busy,
  onStartChat,
  onRefresh,
  onRename,
  onRemove,
  onError,
}: {
  record: ModeAgentServerRecord;
  presentation?: DeviceViewMode;
  busy: boolean;
  onStartChat: (record: ModeAgentServerRecord) => Promise<void>;
  onRefresh: (record: ModeAgentServerRecord) => Promise<void>;
  onRename: (record: ModeAgentServerRecord, name: string) => Promise<void>;
  onRemove: (record: ModeAgentServerRecord) => Promise<void>;
  onError: (message: string) => void;
}) {
  const { intl } = useModeIntl();
  const [starting, setStarting] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [confirmingRemove, setConfirmingRemove] = useState(false);
  const status = modeAgentStatus(record.server.status);
  const address = `${record.host}:${record.port}`;
  const toolCount = record.server.toolCount;
  const detail = [
    typeof toolCount === "number"
      ? intl.formatMessage({ id: "remoteDesktop.devices.mcpTools" }, { count: toolCount })
      : intl.formatMessage({ id: "remoteDesktop.devices.mcp" }),
    record.server.error ?? "",
  ]
    .filter(Boolean)
    .join(" · ");

  const saveRename = async () => {
    const nextName = editing?.trim() ?? "";
    if (!nextName) return;
    try {
      await onRename(record, nextName);
      setEditing(null);
    } catch (error) {
      onError(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <RowShell
      testId="remote-control-mode-agent"
      presentation={presentation}
      editor={
        editing ? (
          <div className="flex flex-wrap items-center gap-2">
            <Input
              size="lg"
              data-testid="remote-control-mode-agent-name-input"
              value={editing}
              onChange={(event) => setEditing(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void saveRename();
                if (event.key === "Escape") setEditing(null);
              }}
            />
            <Button
              type="button"
              size="sm"
              disabled={busy || !editing.trim()}
              onClick={() => void saveRename()}
            >
              <Check className="size-4" />
              {intl.formatMessage({ id: "remoteDesktop.device.save" })}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={busy}
              onClick={() => setEditing(null)}
            >
              {intl.formatMessage({ id: "remoteDesktop.device.cancel" })}
            </Button>
          </div>
        ) : undefined
      }
    >
      <DeviceIdentity
        pill={<StatusPill labelId={status.labelId} tone={status.tone} pulsing={status.pulsing} />}
        name={record.name}
        kindLabelId="remoteDesktop.devices.kind.modeAgent"
        address={address}
        detail={detail}
        addressTestId="remote-control-mode-agent-address"
      />
      {editing ? null : confirmingRemove ? (
        <div className="flex flex-wrap items-center justify-end gap-2">
          <span className="text-ui-xs text-foreground-subtle">
            {intl.formatMessage({ id: "remoteDesktop.devices.removeAsk" }, { name: record.name })}
          </span>
          <Button
            type="button"
            variant="destructive"
            size="sm"
            disabled={busy}
            onClick={() => {
              void (async () => {
                try {
                  await onRemove(record);
                  setConfirmingRemove(false);
                } catch (error) {
                  onError(error instanceof Error ? error.message : String(error));
                }
              })();
            }}
          >
            {intl.formatMessage({ id: "remoteDesktop.modeAgents.removeConfirm" })}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={busy}
            onClick={() => setConfirmingRemove(false)}
          >
            {intl.formatMessage({ id: "remoteDesktop.device.cancel" })}
          </Button>
        </div>
      ) : (
        <div className="flex flex-wrap items-center justify-end gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="remote-control-mode-agent-chat"
            disabled={busy || starting}
            onClick={() => {
              setStarting(true);
              void onStartChat(record)
                .catch((error: unknown) =>
                  onError(error instanceof Error ? error.message : String(error)),
                )
                .finally(() => setStarting(false));
            }}
          >
            {starting ? (
              <LoaderIcon className="size-4 animate-spin" />
            ) : (
              <MessageSquarePlus className="size-4" />
            )}
            {intl.formatMessage({ id: "remoteDesktop.modeAgents.startChat" })}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            data-testid="remote-control-mode-agent-refresh"
            title={intl.formatMessage({ id: "remoteDesktop.devices.refresh" })}
            aria-label={intl.formatMessage({ id: "remoteDesktop.devices.refresh" })}
            disabled={busy || starting || refreshing}
            onClick={() => {
              onError("");
              setRefreshing(true);
              void onRefresh(record)
                .catch((error: unknown) =>
                  onError(error instanceof Error ? error.message : String(error)),
                )
                .finally(() => setRefreshing(false));
            }}
          >
            <RefreshCw className={cn("size-3.5", refreshing && "animate-spin")} />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            data-testid="remote-control-mode-agent-rename"
            title={intl.formatMessage({ id: "remoteDesktop.device.rename" })}
            aria-label={intl.formatMessage({ id: "remoteDesktop.device.rename" })}
            disabled={busy || starting || refreshing}
            onClick={() => {
              onError("");
              setEditing(record.name);
            }}
          >
            <Pencil className="size-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            data-testid="remote-control-mode-agent-remove"
            title={intl.formatMessage({ id: "remoteDesktop.modeAgents.remove" })}
            aria-label={intl.formatMessage({ id: "remoteDesktop.modeAgents.remove" })}
            disabled={busy || starting}
            onClick={() => {
              onError("");
              setConfirmingRemove(true);
            }}
          >
            <Trash2 className="size-3.5" />
          </Button>
        </div>
      )}
    </RowShell>
  );
}
