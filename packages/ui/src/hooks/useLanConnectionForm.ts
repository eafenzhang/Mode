import { useCallback, useEffect, useMemo, useState } from "react";
import type { LanPairedPeer } from "@mode/shared";
import { usePlatform } from "@/hooks/usePlatform.js";

/**
 * 「局域网」连接方式的状态：只负责在**已配对设备**里选一台。
 *
 * 配对、设备发现、手动地址与配对码都留在「远程控制」页（spec:
 * docs/specs/lan-paired-devices.md §向导）。向导不写 `lan:peer:*`，因此这里
 * 只读 `getLanPairedPeers()`，也不能回落到手工地址。
 */
export function useLanConnectionForm(options: { open: boolean; kind: string }) {
  const platform = usePlatform();
  const [pairedPeers, setPairedPeers] = useState<LanPairedPeer[]>([]);
  const [selectedServerId, setSelectedServerId] = useState("");
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState("");

  const reload = useCallback(async () => {
    setLoading(true);
    setLoadError("");
    try {
      setPairedPeers(await platform.getLanPairedPeers());
    } catch (error) {
      setPairedPeers([]);
      setLoadError(error instanceof Error ? error.message : String(error));
    } finally {
      setLoading(false);
    }
  }, [platform]);

  // 只在真正进入局域网页时读一次配对记录；重复打开时刷新，删除后不会留下失效行。
  useEffect(() => {
    if (!options.open || options.kind !== "lan") {
      return;
    }
    void reload();
  }, [options.kind, options.open, reload]);

  // 被删除或刷新后消失的选择不能继续作为提交目标。
  useEffect(() => {
    if (
      selectedServerId &&
      !pairedPeers.some((peer) => peer.serverId === selectedServerId)
    ) {
      setSelectedServerId("");
    }
  }, [pairedPeers, selectedServerId]);

  const selectedPeer = useMemo(
    () => pairedPeers.find((peer) => peer.serverId === selectedServerId) ?? null,
    [pairedPeers, selectedServerId],
  );

  const selectPairedPeer = useCallback((peer: LanPairedPeer) => {
    setSelectedServerId(peer.serverId);
  }, []);

  return {
    lanPairedPeers: pairedPeers,
    lanSelectedPeer: selectedPeer,
    lanPairedSelectedId: selectedServerId,
    lanPairedLoading: loading,
    lanPairedError: loadError,
    loadLanPairedPeers: reload,
    selectPairedPeer,
  };
}
