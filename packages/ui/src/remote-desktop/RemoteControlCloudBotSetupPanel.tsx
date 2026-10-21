import { useEffect, useState } from "react";
import { LoaderIcon, RefreshCw } from "lucide-react";
import type { CloudBotCandidate } from "@mode/shared";
import type { ICloudAccountService } from "@mode/services";
import { Button } from "@/ui-foundation/adapters/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";

/**
 * 把本机一个已 /bind 的微信 Bot 同步成“云端登录 Bot”。
 * 上传成功后 Host 会停用本地 Bot，避免同一个 iLink token 被两边轮询。
 */
export function RemoteControlCloudBotSetupPanel({
  service,
  onSynced,
  onClose,
}: {
  service: ICloudAccountService;
  onSynced: () => void;
  onClose: () => void;
}) {
  const { intl } = useModeIntl();
  const [candidates, setCandidates] = useState<CloudBotCandidate[] | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [syncedId, setSyncedId] = useState<string | null>(null);

  const message = (id: string): string => intl.formatMessage({ id });

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const next = await service.listCloudBotCandidates();
        if (!cancelled) {
          setCandidates(next);
        }
      } catch (caught) {
        if (!cancelled) {
          setError(caught instanceof Error ? caught.message : String(caught));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [service]);

  const sync = async (botId: string): Promise<void> => {
    setBusyId(botId);
    setError("");
    try {
      await service.syncCloudLoginBot({ botId });
      setSyncedId(botId);
      onSynced();
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="mt-2 rounded-lg border border-border bg-background px-3 py-3">
      <p className="text-ui-base text-foreground">{message("remoteDesktop.cloud.setupBot")}</p>
      <p className="mt-1 text-ui-xs text-foreground-subtle">
        {message("remoteDesktop.cloud.setupBotHint")}
      </p>
      {candidates === null ? (
        <p className="mt-2 flex items-center gap-2 text-ui-base text-foreground-subtle">
          <LoaderIcon className="size-4 animate-spin" />
          {message("remoteDesktop.cloud.loading")}
        </p>
      ) : candidates.length === 0 ? (
        <p className="mt-2 text-ui-base text-foreground-subtle">
          {message("remoteDesktop.cloud.noWeixin")}
        </p>
      ) : (
        <ul className="mt-2 space-y-2">
          {candidates.map((candidate) => {
            const ready = Boolean(candidate.boundUser) && candidate.hasToken;
            return (
              <li
                key={candidate.id}
                className="flex items-center justify-between gap-3 rounded-lg border border-border px-3 py-2"
              >
                <div className="min-w-0">
                  <div className="truncate text-ui-base text-foreground">{candidate.name}</div>
                  <div className="text-ui-xs text-foreground-subtle">
                    {candidate.boundUser
                      ? `微信：${candidate.boundUser}`
                      : message("remoteDesktop.cloud.botNeedsBind")}
                    {!candidate.hasToken ? ` · ${message("remoteDesktop.cloud.botNoToken")}` : ""}
                  </div>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={!ready || busyId !== null}
                  onClick={() => void sync(candidate.id)}
                >
                  {busyId === candidate.id ? (
                    <LoaderIcon className="size-4 animate-spin" />
                  ) : (
                    <RefreshCw className="size-4" />
                  )}
                  {message(
                    busyId === candidate.id
                      ? "remoteDesktop.cloud.syncingBot"
                      : "remoteDesktop.cloud.syncBot",
                  )}
                </Button>
              </li>
            );
          })}
        </ul>
      )}
      {syncedId ? (
        <p className="mt-2 text-ui-base text-foreground">
          {message("remoteDesktop.cloud.botSynced")}
        </p>
      ) : null}
      {error ? (
        <p className="mt-2 text-ui-base text-destructive">
          {message("remoteDesktop.cloud.botSetupFailed")}：{error}
        </p>
      ) : null}
      <Button type="button" variant="ghost" size="sm" className="mt-2" onClick={onClose}>
        {message("remoteDesktop.cloud.close")}
      </Button>
    </div>
  );
}
