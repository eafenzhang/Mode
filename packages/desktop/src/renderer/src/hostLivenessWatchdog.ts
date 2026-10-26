/**
 * Host 半开管道心跳（spec `tauri-renderer-host.md` §22.5）。
 *
 * 「进程死了」走 stdout EOF（state stream failed → 既有 fail-closed）；「进程活着但
 * 事件循环停摆」只有**应用层探测**能发现：定期对 host 频道发 `ping` 并限时等 `pong`。
 * 连续两次拿不到 pong → 判定传输停摆，交给 onStalled（kill + fail-closed，走重生换代）。
 *
 * 防误杀：deadline（10s）远大于正常 ping（毫秒级），且要**连续**两次；探测结果只看
 * 「deadline 内有没有 pong」，迟到 pong 不回滚 strike——事件循环停摆 20s 对用户而言
 * 已经等效死亡。探测与业务负载共用事件循环，Host 繁忙时 pong 会慢但不至于超限。
 *
 * 本模块是**纯步进逻辑**：调度器可注入，测试不依赖真实定时器。
 */
export interface HostLivenessWatchdogOptions {
  /** 探测间隔（两次 ping 的发起间距）。 */
  intervalMs: number;
  /** 单次 pong 的 deadline。 */
  deadlineMs: number;
  /** 连续多少次拿不到 pong 判定停摆。 */
  strikesToFail: number;
  /** 活性探测本身；reject/超时都算「没等到 pong」。 */
  ping: () => Promise<void>;
  /** 判定停摆（只回调一次，随后看门狗自停）。 */
  onStalled: () => void;
  /** 每次探测的诊断出口（测试断言用；产品侧可接 logger）。 */
  onProbe?: (ok: boolean, strikes: number) => void;
  /** 可注入调度器：安排一个 `ms` 后的回调，返回取消函数。 */
  schedule?: (callback: () => void, ms: number) => () => void;
}

export interface HostLivenessWatchdog {
  /** 停止探测（fail-closed / 重载前调用；幂等）。 */
  stop(): void;
}

export function startHostLivenessWatchdog(
  options: HostLivenessWatchdogOptions,
): HostLivenessWatchdog {
  const schedule = options.schedule ?? defaultSchedule;
  let stopped = false;
  let strikes = 0;
  let cancelTick: (() => void) | undefined;

  const clearTick = (): void => {
    cancelTick?.();
    cancelTick = undefined;
  };

  const probe = (): void => {
    if (stopped) {
      return;
    }
    let rejectDeadline: ((error: Error) => void) | undefined;
    const cancelDeadline = schedule(() => {
      rejectDeadline?.(new Error("host liveness deadline"));
    }, options.deadlineMs);
    const deadline = new Promise<never>((_, reject) => {
      rejectDeadline = reject;
    });
    void Promise.race([
      options.ping().then(
        () => true,
        () => false,
      ),
      deadline.then(
        () => false,
        () => false,
      ),
    ]).then((alive) => {
      cancelDeadline();
      if (stopped) {
        return;
      }
      if (alive) {
        strikes = 0;
        options.onProbe?.(true, strikes);
        tick();
        return;
      }
      strikes += 1;
      options.onProbe?.(false, strikes);
      if (strikes >= options.strikesToFail) {
        stopped = true;
        options.onStalled();
        return;
      }
      tick();
    });
  };

  const tick = (): void => {
    if (stopped) {
      return;
    }
    cancelTick = schedule(probe, options.intervalMs);
  };

  tick();

  return {
    stop(): void {
      stopped = true;
      clearTick();
    },
  };
}

function defaultSchedule(callback: () => void, ms: number): () => void {
  const timer = setTimeout(callback, ms);
  return () => clearTimeout(timer);
}

/** 产品参数（钉值；spec §22.5）：30s 一探、10s deadline、连续 2 次。 */
export const HOST_LIVENESS_INTERVAL_MS = 30_000;
export const HOST_LIVENESS_DEADLINE_MS = 10_000;
export const HOST_LIVENESS_STRIKES = 2;
