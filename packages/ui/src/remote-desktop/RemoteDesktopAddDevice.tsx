/**
 * 远程控制页 · 添加设备（「检测设备 + 配对」从远程连接向导前移到本页）：
 * - 检测：`discoverLanPeers`（UDP 广播扫描，1~2 秒窗口）；
 * - 配对：选中设备 + 输对端配对码 `pairLanPeer`（令牌由 Host 落凭据服务）；
 * - 成功或已配对的设备不再出现在检测列表（spec UI 规则「设备页」：
 *   已配对 serverId 过滤，全部被过滤时给出「均已配对」提示）。
 */
import { useCallback, useState } from "react";
import {
  AlertTriangleIcon,
  CheckIcon,
  LoaderIcon,
  Plus,
  RefreshCwIcon,
} from "lucide-react";
import type { LanDiscoveredPeer } from "@mode/shared";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { cn } from "@/components/lib/utils.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { filterUnpairedDiscoveredPeers } from "@/remote-desktop/deviceAvailability.js";

export function RemoteDesktopAddDevice({
  onPaired,
  pairedServerIds,
}: {
  onPaired: () => void | Promise<void>;
  /** 已配对设备的主键集合：检测结果里过滤掉（配对成功立即消失，重复检测也不再出现）。 */
  pairedServerIds: string[];
}) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const [expanded, setExpanded] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [scanError, setScanError] = useState("");
  const [peers, setPeers] = useState<LanDiscoveredPeer[]>([]);
  const [selected, setSelected] = useState<LanDiscoveredPeer | null>(null);
  const [code, setCode] = useState("");
  const [pairing, setPairing] = useState(false);
  const [pairError, setPairError] = useState("");

  const scan = useCallback(async () => {
    setScanning(true);
    setScanError("");
    try {
      setPeers(await platform.discoverLanPeers());
    } catch (error) {
      setPeers([]);
      setScanError(error instanceof Error ? error.message : String(error));
    } finally {
      setScanning(false);
    }
  }, [platform]);

  const toggle = useCallback(() => {
    setExpanded((previous) => {
      const next = !previous;
      // 首次展开即检测一次；此后由「重新检测」按钮驱动（不自动重复扫描）。
      if (next && peers.length === 0 && !scanning) {
        void scan();
      }
      return next;
    });
  }, [peers.length, scan, scanning]);

  const pair = useCallback(async () => {
    if (!selected) {
      return;
    }
    setPairing(true);
    setPairError("");
    try {
      await platform.pairLanPeer({
        host: selected.host,
        port: selected.port,
        code: code.trim().toUpperCase(),
      });
      setPeers((previous) => previous.filter((peer) => peer.serverId !== selected.serverId));
      setSelected(null);
      setCode("");
      await onPaired();
    } catch (error) {
      setPairError(error instanceof Error ? error.message : String(error));
    } finally {
      setPairing(false);
    }
  }, [code, onPaired, platform, selected]);

  // 已配对设备从检测列表过滤（配对成功与「本已配对后重新检测」两个入口统一在此收口）。
  const { visible, hiddenCount } = filterUnpairedDiscoveredPeers(peers, pairedServerIds);

  return (
    <div className="rounded-lg border border-border bg-card px-4 py-3">
      <div className="flex items-center justify-between gap-2">
        <span className="text-ui-base text-foreground">
          {intl.formatMessage({ id: "remoteDesktop.addDevice.title" })}
        </span>
        <Button type="button" variant={expanded ? "ghost" : "outline"} size="sm" onClick={toggle}>
          <Plus className="size-4" />
          {intl.formatMessage({
            id: expanded ? "remoteDesktop.addDevice.collapse" : "remoteDesktop.addDevice.detect",
          })}
        </Button>
      </div>
      {expanded ? (
        <div className="mt-3 space-y-3">
          <div className="flex items-center justify-between gap-2">
            <span className="text-ui-base text-foreground-subtle">
              {intl.formatMessage({ id: "remote.lan.discoveredTitle" })}
            </span>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 gap-1.5 px-2"
              disabled={scanning}
              onClick={() => void scan()}
            >
              {scanning ? (
                <LoaderIcon className="size-3.5 animate-spin" />
              ) : (
                <RefreshCwIcon className="size-3.5" />
              )}
              {intl.formatMessage({ id: "remote.lan.rescan" })}
            </Button>
          </div>
          {visible.length > 0 ? (
            <div className="space-y-2">
              {visible.map((peer) => {
                const isSelected = selected?.serverId === peer.serverId;
                return (
                  <div key={peer.serverId} className="space-y-2">
                    <button
                      type="button"
                      onClick={() => {
                        setSelected(isSelected ? null : peer);
                        setPairError("");
                      }}
                      className={cn(
                        "flex w-full items-center justify-between gap-3 rounded-lg border px-3 py-2 text-start transition-colors",
                        isSelected
                          ? "border-input-border-focused bg-surface-hover"
                          : "border-card-border bg-card hover:border-input-border-hover hover:bg-surface-hover",
                      )}
                    >
                      <span className="min-w-0">
                        <span className="block truncate text-ui-base text-foreground">
                          {peer.name?.trim() || peer.serverId}
                        </span>
                        <span className="block truncate text-ui-xs text-foreground-subtle">
                          {`${peer.host}:${peer.port}`}
                        </span>
                      </span>
                      {isSelected ? <CheckIcon className="size-4 shrink-0 text-primary" /> : null}
                    </button>
                    {isSelected ? (
                      <div className="space-y-2 rounded-lg border border-dashed border-border px-3 py-2">
                        <div className="flex items-center gap-2">
                          <Input
                            size="lg"
                            className="max-w-40 tracking-[0.3em] uppercase"
                            data-testid="remote-desktop-pair-code-input"
                            value={code}
                            onChange={(event) =>
                              setCode(
                                event.target.value
                                  .replace(/[^0-9a-fA-F]/gu, "")
                                  .slice(0, 6)
                                  .toUpperCase(),
                              )
                            }
                            placeholder="A1B2C3"
                          />
                          <Button
                            type="button"
                            variant="outline"
                            size="lg"
                            data-testid="remote-desktop-pair-button"
                            disabled={pairing || code.trim().length < 4}
                            onClick={() => void pair()}
                          >
                            {pairing ? <LoaderIcon className="size-3.5 animate-spin" /> : null}
                            {intl.formatMessage({ id: "remote.lan.pair" })}
                          </Button>
                        </div>
                        <p className="text-ui-xs text-foreground-subtlest">
                          {intl.formatMessage({ id: "remote.lan.pairCodeHint" })}
                        </p>
                      </div>
                    ) : null}
                  </div>
                );
              })}
            </div>
          ) : (
            <p className="rounded-lg border border-dashed border-border px-3 py-2 text-ui-base text-foreground-subtle">
              {scanning
                ? intl.formatMessage({ id: "remote.lan.scanning" })
                : hiddenCount > 0
                  ? intl.formatMessage(
                      { id: "remoteDesktop.addDevice.pairedHidden" },
                      { count: String(hiddenCount) },
                    )
                  : intl.formatMessage({ id: "remote.lan.empty" })}
            </p>
          )}
          {visible.length > 0 && hiddenCount > 0 ? (
            <p className="text-ui-xs text-foreground-subtlest">
              {intl.formatMessage(
                { id: "remoteDesktop.addDevice.pairedHidden" },
                { count: String(hiddenCount) },
              )}
            </p>
          ) : null}
          {scanError ? (
            <p className="flex items-center gap-1.5 text-ui-base text-warning">
              <AlertTriangleIcon className="size-3.5 shrink-0" />
              {scanError}
            </p>
          ) : null}
          {pairError ? (
            <p className="flex items-center gap-1.5 text-ui-base text-destructive">
              <AlertTriangleIcon className="size-3.5 shrink-0" />
              {pairError}
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
