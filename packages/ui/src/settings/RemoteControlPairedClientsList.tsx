/**
 * 设置页「远程控制 · 已配对设备」列表（从 RemoteControlSettingsSection 拆出，max-lines 约束）：
 * 行 = LAN 配对记录本身——名称 / 最近使用 / 最近工作区 / 远程桌面授权开关
 * （写记录上的 `remoteDesktopAuthorized`，与同意窗「记住此设备」同字段）/ 移除；
 * 底部「重置全部令牌」。纯展示 + 回调，状态与写路径都由父组件持有。
 * 规格：docs/specs/lan-remote-desktop.md UI 规则「被控端」、docs/specs/lan-paired-devices.md。
 */
import { ShieldOff, Trash2 } from "lucide-react";
import type { LanAccessClientRecord } from "@mode/shared";
import { Button } from "@/components/ui/button.js";
import { Switch } from "@/components/ui/switch.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";

export function RemoteControlPairedClientsList({
  clients,
  busy,
  onSetAuthorized,
  onRemove,
  onResetTokens,
}: {
  clients: LanAccessClientRecord[];
  busy: boolean;
  onSetAuthorized: (client: LanAccessClientRecord, authorized: boolean) => void;
  onRemove: (clientId: string) => void;
  onResetTokens: () => void;
}) {
  const { intl } = useModeIntl();
  return (
    <div className="space-y-2">
      <ul className="space-y-2" data-testid="remote-desktop-paired-clients">
        {clients.map((client) => {
          const name = client.label || client.id;
          const authorized = client.remoteDesktopAuthorized === true;
          return (
            <li
              key={client.id}
              className="flex items-start justify-between gap-3 rounded-lg border border-border bg-card px-3 py-2"
            >
              <span className="min-w-0 flex-1">
                <span className="block truncate text-ui-base text-foreground">{name}</span>
                <span className="block text-ui-xs text-foreground-subtle">
                  {client.lastUsedAt
                    ? intl.formatMessage(
                        { id: "settings.lanAccess.clientLastUsed" },
                        { time: new Date(client.lastUsedAt).toLocaleString() },
                      )
                    : intl.formatMessage({ id: "settings.lanAccess.clientNeverUsed" })}
                </span>
                <span className="block truncate text-ui-xs text-foreground-subtle">
                  {client.lastWorkspacePath
                    ? intl.formatMessage(
                        { id: "settings.lanAccess.lastWorkspace" },
                        { path: client.lastWorkspacePath },
                      )
                    : intl.formatMessage({ id: "settings.lanAccess.noLastWorkspace" })}
                </span>
                <span className="mt-0.5 block text-ui-xs text-foreground-subtle">
                  {intl.formatMessage({
                    id: authorized
                      ? "settings.lanAccess.rdAuthorized"
                      : "settings.lanAccess.rdUnauthorized",
                  })}
                </span>
              </span>
              <span className="flex shrink-0 items-center gap-1.5">
                <Switch
                  checked={authorized}
                  disabled={busy}
                  aria-label={intl.formatMessage(
                    {
                      id: authorized
                        ? "settings.lanAccess.rdRevokeFor"
                        : "settings.lanAccess.rdAuthorizeFor",
                    },
                    { name },
                  )}
                  onCheckedChange={(checked) => onSetAuthorized(client, checked)}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={busy}
                  aria-label={intl.formatMessage(
                    { id: "settings.lanAccess.removeClient" },
                    { name },
                  )}
                  onClick={() => onRemove(client.id)}
                >
                  <Trash2 className="size-4" />
                </Button>
              </span>
            </li>
          );
        })}
      </ul>
      <div className="flex justify-end">
        <Button type="button" variant="ghost" size="sm" disabled={busy} onClick={onResetTokens}>
          <ShieldOff className="size-4" />
          {intl.formatMessage({ id: "settings.lanAccess.resetTokens" })}
        </Button>
      </div>
    </div>
  );
}
