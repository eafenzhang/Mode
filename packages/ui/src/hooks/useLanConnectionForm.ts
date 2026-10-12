import { useCallback, useEffect, useState } from "react";
import type { LanDiscoveredPeer, LanPairedPeer } from "@mode/shared";
import { usePlatform } from "@/hooks/usePlatform.js";

/**
 * 「局域网」连接方式的表单状态：设备发现、地址与配对。
 * 单独成 hook 而不是塞进 useRemoteConnectionForm：那片状态已经逼近文件行数上限，
 * 且发现的扫描窗口与配对换令牌都有自己的一次性副作用。
 */
export function useLanConnectionForm(options: { open: boolean; kind: string }) {
  const platform = usePlatform();
  const [lanHost, setLanHost] = useState("");
  const [lanPort, setLanPort] = useState("");
  const [lanPairCode, setLanPairCode] = useState("");
  const [lanServerId, setLanServerId] = useState("");
  const [lanServerName, setLanServerName] = useState("");
  const [lanToken, setLanToken] = useState("");
  const [lanPeers, setLanPeers] = useState<LanDiscoveredPeer[]>([]);
  const [lanPairedPeers, setLanPairedPeers] = useState<LanPairedPeer[]>([]);
  const [lanPairedSelectedId, setLanPairedSelectedId] = useState("");
  const [lanScanning, setLanScanning] = useState(false);
  const [lanScanError, setLanScanError] = useState("");
  const [lanPairing, setLanPairing] = useState(false);
  const [lanPairError, setLanPairError] = useState("");

  const scanLanPeers = useCallback(async () => {
    setLanScanning(true);
    setLanScanError("");
    try {
      setLanPeers(await platform.discoverLanPeers());
    } catch (error) {
      setLanPeers([]);
      setLanScanError(error instanceof Error ? error.message : String(error));
    } finally {
      setLanScanning(false);
    }
  }, [platform]);

  /** 已配对设备（凭据服务元数据；令牌不出 host）：向导里首选直连，免配对码。 */
  const loadLanPairedPeers = useCallback(async () => {
    try {
      setLanPairedPeers(await platform.getLanPairedPeers());
    } catch {
      setLanPairedPeers([]); // 读取失败不打断手工填写/配对路径
    }
  }, [platform]);

  // 只在真正进入局域网页时扫一次；被防火墙拦住时用户还能手动填地址。
  useEffect(() => {
    if (!options.open || options.kind !== "lan") {
      return;
    }
    void scanLanPeers();
    void loadLanPairedPeers();
  }, [options.kind, options.open, scanLanPeers, loadLanPairedPeers]);

  /** 配对：用对端显示的配对码换长期令牌（令牌由 Host 侧写凭据服务，这里只留内存副本）。 */
  const pairLanWithCode = useCallback(async () => {
    const targetHost = lanHost.trim();
    const targetPort = Number.parseInt(lanPort.trim(), 10);
    if (!targetHost || !Number.isFinite(targetPort) || targetPort <= 0 || targetPort > 65535) {
      setLanPairError("请先填写对端地址与端口");
      return;
    }
    if (!lanPairCode.trim()) {
      setLanPairError("请输入对端显示的配对码");
      return;
    }
    setLanPairing(true);
    setLanPairError("");
    try {
      const result = await platform.pairLanPeer({
        host: targetHost,
        port: targetPort,
        code: lanPairCode.trim(),
        // 不传 label：host 侧缺省兜底本机主机名。此前传 navigator.platform
        // （Windows 上是 "Win32"），对端「配对我的设备」列表会显示 win32。
      });
      setLanServerId(result.serverId);
      setLanServerName(result.name ?? "");
      setLanToken(result.token);
      setLanPairedSelectedId(""); // 新配对完成：脱离「已配对直选」上下文并刷新列表
      void loadLanPairedPeers();
    } catch (error) {
      setLanToken("");
      setLanPairError(error instanceof Error ? error.message : String(error));
    } finally {
      setLanPairing(false);
    }
  }, [lanHost, lanPairCode, lanPort, platform, loadLanPairedPeers]);

  /** 选中发现列表里的设备：填入地址端口，并清掉上一台的配对结果。 */
  const selectLanPeer = useCallback((peer: LanDiscoveredPeer) => {
    setLanHost(peer.host);
    setLanPort(String(peer.port));
    setLanServerId(peer.serverId);
    setLanServerName(peer.name ?? "");
    setLanToken("");
    setLanPairError("");
    setLanPairedSelectedId("");
  }, []);

  /** 选中「已配对设备」：地址/身份直接填入；令牌由 Host 按 serverId 从凭据服务取，
   *  无需再走配对码（连接行为与配对后一致）。 */
  const selectPairedPeer = useCallback((peer: LanPairedPeer) => {
    setLanHost(peer.host);
    setLanPort(String(peer.port));
    setLanServerId(peer.serverId);
    setLanServerName(peer.name ?? "");
    setLanToken("");
    setLanPairError("");
    setLanPairedSelectedId(peer.serverId);
  }, []);

  /** 手工改动地址/端口即视为脱离「已配对直选」上下文（清徽标；配对码区按需回归）。 */
  const setLanHostWrapped = useCallback((value: string) => {
    setLanHost(value);
    setLanPairedSelectedId("");
  }, []);
  const setLanPortWrapped = useCallback((value: string) => {
    setLanPort(value);
    setLanPairedSelectedId("");
  }, []);

  return {
    lanHost,
    lanPort,
    lanPairCode,
    lanServerId,
    lanServerName,
    lanToken,
    lanPeers,
    lanPairedPeers,
    lanPairedSelectedId,
    lanScanning,
    lanScanError,
    lanPairing,
    lanPairError,
    setLanHost: setLanHostWrapped,
    setLanPort: setLanPortWrapped,
    setLanPairCode,
    setLanServerId,
    setLanServerName,
    setLanToken,
    scanLanPeers,
    loadLanPairedPeers,
    pairLanWithCode,
    selectLanPeer,
    selectPairedPeer,
  };
}
