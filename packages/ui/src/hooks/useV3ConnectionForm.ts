import { useCallback, useEffect, useMemo, useState } from "react";
import type { RemoteV3DeviceSummary } from "@mode/shared";

import { useServices } from "@/hooks/useServices.js";

/** 远程工作区向导的 v3 设备选择：只读账号设备目录，连接由 Host 的 v3 服务完成。 */
export function useV3ConnectionForm(options: { open: boolean; kind: string }) {
  const services = useServices();
  const remoteV3 = services.remoteV3Service;
  const [hosts, setHosts] = useState<RemoteV3DeviceSummary[]>([]);
  const [selectedDeviceId, setSelectedDeviceId] = useState("");
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState("");

  const reload = useCallback(async () => {
    if (!remoteV3) {
      setHosts([]);
      setLoadError("");
      return;
    }
    setLoading(true);
    setLoadError("");
    try {
      setHosts(await remoteV3.listHosts());
    } catch (error) {
      setHosts([]);
      setLoadError(error instanceof Error ? error.message : String(error));
    } finally {
      setLoading(false);
    }
  }, [remoteV3]);

  useEffect(() => {
    if (!options.open || options.kind !== "v3") {
      return;
    }
    void reload();
  }, [options.kind, options.open, reload]);

  // 设备下线或目录刷新后消失的选择不能继续作为提交目标。
  useEffect(() => {
    if (selectedDeviceId && !hosts.some((host) => host.deviceId === selectedDeviceId)) {
      setSelectedDeviceId("");
    }
  }, [hosts, selectedDeviceId]);

  const selectedHost = useMemo(
    () => hosts.find((host) => host.deviceId === selectedDeviceId) ?? null,
    [hosts, selectedDeviceId],
  );

  const selectHost = useCallback((host: RemoteV3DeviceSummary) => {
    setSelectedDeviceId(host.deviceId);
  }, []);

  return {
    v3Hosts: hosts,
    v3SelectedHost: selectedHost,
    v3SelectedDeviceId: selectedDeviceId,
    v3HostsLoading: loading,
    v3HostsError: loadError,
    v3Available: Boolean(remoteV3),
    loadV3Hosts: reload,
    selectV3Host: selectHost,
  };
}
