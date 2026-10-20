import { useCallback, useState } from "react";
import type { LanDiscoveredPeer } from "@mode/shared";
import { usePlatform } from "@/hooks/usePlatform.js";

/**
 * 远程控制页的「发现的未配对 LAN 设备」：页级瞬态，不持久化。
 * 扫描一次 → 过滤已忽略项 → 配对成功或忽略后从列表移除。
 */
export function useDiscoveredLanPeers(onPaired: () => Promise<void>) {
  const platform = usePlatform();
  const [peers, setPeers] = useState<LanDiscoveredPeer[]>([]);
  const [dismissed, setDismissed] = useState<Set<string>>(new Set());
  const [scanning, setScanning] = useState(false);
  const [scanError, setScanError] = useState("");

  const scan = useCallback(async () => {
    setScanning(true);
    setScanError("");
    try {
      const list = await platform.discoverLanPeers();
      setPeers(list.filter((peer) => !dismissed.has(peer.serverId)));
    } catch (error) {
      setPeers([]);
      setScanError(error instanceof Error ? error.message : String(error));
    } finally {
      setScanning(false);
    }
  }, [dismissed, platform]);

  const dismiss = useCallback((serverId: string) => {
    setDismissed((previous) => new Set(previous).add(serverId));
    setPeers((previous) => previous.filter((peer) => peer.serverId !== serverId));
  }, []);

  const pair = useCallback(
    async (peer: LanDiscoveredPeer, code: string) => {
      await platform.pairLanPeer({ host: peer.host, port: peer.port, code });
      setPeers((previous) => previous.filter((entry) => entry.serverId !== peer.serverId));
      await onPaired();
    },
    [onPaired, platform],
  );

  return { peers, scanning, scanError, scan, dismiss, pair };
}
