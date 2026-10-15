/**
 * 设置页「远程控制 · 配对过本机的设备」列表（纯展示 + 回调，状态与写路径由父组件持有）。
 *
 * 行 = LAN 配对记录本身：名称 / 最近使用 / 最近工作区 / 移除；底部「重置全部令牌」。
 * **逐设备远程桌面授权开关已删除**（spec: docs/specs/remote-desktop-system-rdp.md 产品规则 6）：
 * 系统 RDP 下授权发生在 Windows 登录，本应用没有执行点，保留开关即为假控制。
 */
import { ShieldOff, Trash2 } from "lucide-react";
import type { LanAccessClientRecord } from "@mode/shared";
import { Button } from "@/components/ui/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";

export function RemoteControlPairedClientsList({
  clients,
  busy,
  onRemove,
  onResetTokens,
}: {
  clients: LanAccessClientRecord[];
  busy: boolean;
  onRemove: (clientId: string) => void;
  onResetTokens: () => void;
}) {
  const { intl } = useModeIntl();
  return (
    <div className="space-y-2">
      <ul className="space-y-2" data-testid="remote-desktop-paired-clients">
        {clients.map((client) => {
          const name = client.label || client.id;
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
              </span>
              <span className="flex shrink-0 items-center gap-1.5">
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
