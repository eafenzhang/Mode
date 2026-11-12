import type { WebAccountSummary } from "@mode/remote-core/web";

/** `/v3` 登录前的已保存账号切换（每个服务地址一份设备身份，互不覆盖）。 */
export function SavedV3Accounts({
  accounts,
  disabled,
  onUse,
  onDelete,
}: {
  accounts: WebAccountSummary[];
  disabled: boolean;
  onUse: (serverUrl: string) => void;
  onDelete?: (serverUrl: string) => void;
}) {
  if (accounts.length === 0) return null;
  return (
    <div className="space-y-2 rounded-lg border border-border bg-background p-3">
      <p className="text-ui-xs text-foreground-subtle">已保存的账号</p>
      <ul className="space-y-1">
        {accounts.map((account) => (
          <li key={account.serverUrl} className="flex items-center justify-between gap-2">
            <div className="min-w-0">
              <p className="truncate text-ui-base text-foreground">{account.serverUrl}</p>
              <p className="truncate text-ui-xs text-foreground-subtle">{account.deviceName}</p>
            </div>
            <div className="flex shrink-0 gap-1.5">
              <button
                type="button"
                className="rounded-lg border border-border px-3 py-1.5 text-ui-sm hover:bg-surface-hover disabled:opacity-50"
                disabled={disabled}
                onClick={() => onUse(account.serverUrl)}
              >
                使用
              </button>
              {onDelete ? (
                <button
                  type="button"
                  className="rounded-lg border border-border px-3 py-1.5 text-ui-sm text-destructive hover:bg-surface-hover disabled:opacity-50"
                  disabled={disabled}
                  onClick={() => onDelete(account.serverUrl)}
                >
                  删除
                </button>
              ) : null}
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}
