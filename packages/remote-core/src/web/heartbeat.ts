/** 通用的链路心跳定时器；返回停止函数。 */
export function startHeartbeatLoop(params: {
  intervalMs: number;
  sendPing: () => void;
  onFail: () => void;
  isClosed: () => boolean;
}): () => void {
  const timer = setInterval(() => {
    if (params.isClosed()) return;
    try {
      params.sendPing();
    } catch {
      params.onFail();
    }
  }, params.intervalMs);
  return () => clearInterval(timer);
}
