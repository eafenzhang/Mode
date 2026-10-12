/**
 * 主控远程桌面会话（信令 + 原生媒体面 + 输入上行 + 统计）。
 *
 * 分工（docs/specs/lan-remote-desktop-native-media.md 状态所有者表）：
 * - main：票据请求、WS 信令转发、会话收口（T3 controllerSession）；
 * - 本 hook（renderer）：原生媒体面会话（media WS + 逐帧解密 + 硬解直画）、输入采集、fps/RTT 统计；
 * - 视图状态机：`sessionReducer.ts`（纯函数，可单测），本 hook 只解释执行 effect。
 *
 * 主控 = 被动收流方：
 *   WS connected → hello → monitors → select-monitor → media-open{key,salt} → 连 media WS。
 * media-open（契约 D）携带加媒体面的 key/salt；连接地址（mediaTicket）不经信令，由 rdStartSession
 * 返回值直达 renderer（契约 E）。输入不再走 DataChannel：与信令同源的 media WS 上行 JSON（契约 B）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import type {
  RemoteDesktopControlEvent,
  RemoteDesktopPointerEvent,
  RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";
import { usePlatform } from "@/hooks/usePlatform.js";
import { logger } from "@/logger.js";
import { attachRemoteDesktopInput, type RemoteDesktopInputCapture } from "./InputCapture.js";
import { createNativeMediaSession, type NativeMediaSession } from "./nativeMediaSession.js";
import {
  initialAdaptiveBitrateState,
  stepAdaptiveBitrate,
} from "./adaptiveBitrate.js";
import {
  createNativeSessionStatsCollector,
  EMPTY_REMOTE_DESKTOP_SESSION_STATS,
  type RemoteDesktopSessionStats,
} from "./sessionStats.js";
import {
  INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE,
  reduceRemoteDesktopSession,
  type RemoteDesktopSessionEffect,
  type RemoteDesktopSessionEvent,
  type RemoteDesktopSessionViewState,
} from "./sessionReducer.js";

/** 统计刷新节拍（spec UI 规则：状态条 3s 刷新）；轮询 effect 按此节拍采样 nativeStats 并发 ping。 */
const REMOTE_DESKTOP_STATS_INTERVAL_MS = 3000;

export interface UseRemoteDesktopSessionResult {
  view: RemoteDesktopSessionViewState;
  stats: RemoteDesktopSessionStats;
  /** 画面容器：键盘焦点与输入采集挂点。 */
  containerRef: React.RefObject<HTMLDivElement | null>;
  /** 原生媒体画面（解码即画）：输入坐标映射的参照面。 */
  canvasRef: React.RefObject<HTMLCanvasElement | null>;
  selectMonitor: (displayId: string) => void;
  /** 本端主动断开（先带出 bye，等 main 收口）。 */
  disconnect: () => void;
}

export function useRemoteDesktopSession(options: {
  serverId: string;
}): UseRemoteDesktopSessionResult {
  const { serverId } = options;
  const platform = usePlatform();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  // 原生媒体面会话：open-media 时建、失败/cleanup 路径收（stop 幂等）。
  const sessionRef = useRef<NativeMediaSession | null>(null);
  // 状态条读数采集器（Task 11 起替代 getStats）：fps 由 onDecoded 计数、RTT 由 pong、分辨率由
  // decoder 配置；3s 轮询采样。放 ref 里保证与 media 会话回调同生命周期、不随渲染重建。
  const nativeStats = useRef(createNativeSessionStatsCollector());
  // mediaUrl（mediaTicket 直连地址）：建连 effect 从 rdStartSession 结果缓存，open-media 时取用。
  const mediaUrlRef = useRef<string | null>(null);
  // 视图状态放在 ref + state 双持：reducer updater 不能被 React 重复调用（StrictMode 下
  // 会产生重复信令），因此 effect 在事件处理点上精确执行一次。
  const viewRef = useRef<RemoteDesktopSessionViewState>(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE);
  const [view, setView] = useState<RemoteDesktopSessionViewState>(
    INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE,
  );
  const [stats, setStats] = useState<RemoteDesktopSessionStats>(EMPTY_REMOTE_DESKTOP_SESSION_STATS);
  // 自适应码率 v1（P1-2）：pong 节拍上采样 RTT + 上拍以来的解码丢弃数 → 两档 + 滞回
  // → 下行 {t:"set-bitrate"}（中继 → helper → ICodecAPI MeanBitRate）。
  const adaptiveRef = useRef(initialAdaptiveBitrateState());
  const droppedSinceAdaptiveSample = useRef(0);

  const sendSignal = useCallback(
    (message: RemoteDesktopSignalMessage) => {
      platform.rdSendSignal(message);
    },
    [platform],
  );

  const applyEffect = useCallback(
    (effect: RemoteDesktopSessionEffect): void => {
      switch (effect.kind) {
        case "signal":
          sendSignal(effect.message);
          return;
        case "end-session":
          void platform.rdEndSession(effect.reason).catch(() => undefined);
          return;
        case "open-media": {
          const canvas = canvasRef.current;
          const mediaUrl = mediaUrlRef.current;
          // 画布由会话面在首帧无条件渲染，本 effect 只可能在挂载（refs 已附着）后、且
          // rdStartSession 已解析（mediaUrl 已缓存）时执行——media-open 必经一轮信令往返。
          if (!canvas || !mediaUrl) {
            dispatchRef.current({ type: "media", status: "failed" });
            return;
          }
          try {
            sessionRef.current = createNativeMediaSession({
              url: mediaUrl,
              // 信令里 key/salt 是十六进制字符串；Task 9 胶水的入参按字节数校验（32B key / 8B salt）。
              keyHex: effect.key,
              saltHex: effect.salt,
              canvas,
              // 分辨率读数：collector 记最近一次配置（随轮询采样进状态条），setStats 保留即时
              // 展示（首帧立即出分辨率，不必等到下一拍）。
              onConfigured: (info) => {
                nativeStats.current.onConfigured(info.width, info.height);
                setStats((previous) => ({ ...previous, width: info.width, height: info.height }));
              },
              onDecoded: () => {
                // fps = 实际绘制帧率：解码即画，每帧计一次；轮询采样时按间隔增量出读数。
                nativeStats.current.onDecoded();
              },
              onFrameDropped: () => {
                // 解码队列门控丢弃：累计到下一次自适应采样（与 RTT 同拍判定）。
                droppedSinceAdaptiveSample.current += 1;
              },
              // 传输断开（连不上 / 中途关闭 / 解码器错误）：按媒体失败收口，不留半开状态。
              onTransportClosed: () => dispatchRef.current({ type: "media", status: "failed" }),
            });
          } catch (error) {
            // 构造期校验失败（非法 key/salt、取不到 2d 上下文）：归因到媒体失败路径，避免把
            // 异常抛回 main 的事件推送回调里。
            logger.warn(
              "[remote-desktop] 原生媒体会话创建失败:",
              error instanceof Error ? error.message : String(error),
            );
            dispatchRef.current({ type: "media", status: "failed" });
            return;
          }
          dispatchRef.current({ type: "media", status: "live" });
          return;
        }
      }
    },
    [platform, sendSignal],
  );
  const applyEffectRef = useRef(applyEffect);
  applyEffectRef.current = applyEffect;

  const dispatch = useCallback((event: RemoteDesktopSessionEvent): void => {
    const transition = reduceRemoteDesktopSession(viewRef.current, event);
    if (transition.state !== viewRef.current) {
      viewRef.current = transition.state;
      setView(transition.state);
    }
    for (const effect of transition.effects) {
      applyEffectRef.current(effect);
    }
  }, []);
  const dispatchRef = useRef(dispatch);
  dispatchRef.current = dispatch;

  // ---- 会话生命周期：订阅 main 事件 → 发起会话（拿 mediaUrl）→ 离开时收口 ----
  useEffect(() => {
    let disposed = false;
    const unsubscribe = platform.onRemoteDesktopSessionEvent((event) => {
      dispatchRef.current({ type: "controller", event });
    });

    void (async () => {
      try {
        const result = await platform.rdStartSession(serverId);
        if (disposed) {
          return;
        }
        if ("error" in result) {
          dispatchRef.current({
            type: "controller",
            event: { kind: "state", state: "closed", error: result.error },
          });
          return;
        }
        mediaUrlRef.current = result.mediaUrl ?? null;
        if (!result.mediaUrl) {
          // 契约 E 未给出连接地址（旧 main / 未贯通）：媒体面无法建立，立即按不支持收口。
          dispatchRef.current({
            type: "controller",
            event: { kind: "state", state: "closed", error: "unsupported" },
          });
        }
      } catch (error) {
        logger.warn(
          "[remote-desktop] 发起会话失败:",
          error instanceof Error ? error.message : String(error),
        );
        if (!disposed) {
          dispatchRef.current({
            type: "controller",
            event: { kind: "state", state: "closed", error: "internal" },
          });
        }
      }
    })();

    return () => {
      disposed = true;
      unsubscribe();
      // 先断媒体面再收口：会话面卸载即停止解密/绘制，stop 幂等（内部 teardown 只通知一次）。
      sessionRef.current?.stop();
      sessionRef.current = null;
      mediaUrlRef.current = null;
      // 离开页面即收口会话：main 侧幂等（对端已 bye / 通道已断时无副作用）。
      // 已收口的会话（对端 bye / 媒体失败 / 本地断开）不重复归因：卸载兜底只服务仍在进行的会话，
      // 避免给已按 error/remote 归因结束的会话再补一次 user-end（台账 Task 10 项）。
      if (viewRef.current.phase !== "closed") {
        void platform.rdEndSession("user-end").catch(() => undefined);
      }
    };
  }, [platform, serverId]);

  // ---- media WS 下行 JSON 控制事件（Task 9 胶水派发到 window，与信令面解耦）----
  useEffect(() => {
    const onControl = (event: Event) => {
      const detail = (event as CustomEvent<{ t?: string; ts?: number; reason?: string }>).detail;
      if (detail?.t === "pong" && typeof detail.ts === "number") {
        // RTT = 收到时刻 - ping 时间戳；异步写入 collector，随下一拍 3s 采样进入状态条。
        const rttMs = Date.now() - detail.ts;
        nativeStats.current.onPong(rttMs);
        // 自适应码率判定（与 RTT 同拍；丢弃数取自上拍以来累计，采样后清零）。
        const outcome = stepAdaptiveBitrate(
          adaptiveRef.current,
          { rttMs, droppedFrames: droppedSinceAdaptiveSample.current },
          Date.now(),
        );
        adaptiveRef.current = outcome.state;
        droppedSinceAdaptiveSample.current = 0;
        if (outcome.nextBps !== null) {
          sessionRef.current?.send({ t: "set-bitrate", bps: outcome.nextBps });
        }
        return;
      }
      // 冻结状态与对端 freeze/unfreeze 信令共用转移（reducer 一个 frozen 布尔）：
      // 两条入口谁先到都行，重复到达幂等（同一状态再次写入无副作用）。
      if (detail?.t === "freeze") {
        dispatchRef.current({ type: "freeze" });
      }
      if (detail?.t === "unfreeze") {
        dispatchRef.current({ type: "unfreeze" });
      }
    };
    window.addEventListener("rd-media-control", onControl);
    return () => window.removeEventListener("rd-media-control", onControl);
  }, []);

  // ---- 状态条读数轮询：3s 采样 collector（fps 增量/RTT/分辨率）并主动 ping 量 RTT ----
  useEffect(() => {
    const timer = window.setInterval(() => {
      setStats(nativeStats.current.sample());
      // 每拍独立发一次 ping（不等 pong 回来再发）；无媒体会话时 ping 是 no-op。
      sessionRef.current?.ping(Date.now());
    }, REMOTE_DESKTOP_STATS_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, []);

  // ---- 输入采集：容器焦点 + 画面 pointer/keyboard → media WS 上行 ----
  useEffect(() => {
    const container = containerRef.current;
    const canvas = canvasRef.current;
    if (!container || !canvas) {
      return;
    }
    // 输入不再分双通道：control 与 pointer 都走同一条 media WS（webSocket send 在未 OPEN 时
    // 静默丢弃——pointer 的「允许丢」语义由传输层天然覆盖，无需排队或重试）。
    const sink = {
      sendControl(event: RemoteDesktopControlEvent): void {
        sessionRef.current?.send(event);
      },
      sendPointer(event: RemoteDesktopPointerEvent): void {
        sessionRef.current?.send(event);
      },
    };
    const capture: RemoteDesktopInputCapture = attachRemoteDesktopInput({
      container,
      surface: canvas,
      sink,
    });
    // 进入会话面即聚焦：键盘事件无需先点击画面。
    container.focus({ preventScroll: true });
    return () => capture.detach();
  }, []);

  const selectMonitor = useCallback((displayId: string) => {
    dispatchRef.current({ type: "select-monitor", displayId });
  }, []);

  const disconnect = useCallback(() => {
    dispatchRef.current({ type: "local-end" });
  }, []);

  return {
    view,
    stats,
    containerRef,
    canvasRef,
    selectMonitor,
    disconnect,
  };
}
