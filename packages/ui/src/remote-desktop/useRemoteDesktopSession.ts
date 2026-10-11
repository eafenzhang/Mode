/**
 * 主控远程桌面会话（信令 + WebRTC 媒体 + 输入通道 + 统计）。
 *
 * 分工（docs/specs/lan-remote-desktop.md 状态所有者表）：
 * - main：票据请求、WS 信令转发、会话收口（T3 controllerSession）；
 * - 本 hook（renderer）：RTCPeerConnection、DataChannel、输入采集、fps/RTT 统计；
 * - 视图状态机：`sessionReducer.ts`（纯函数，可单测），本 hook 只解释执行 effect。
 *
 * 主控 = answer 方（被控端在 select-monitor 后起采集并发 offer）：
 *   WS connected → hello → monitors → select-monitor → offer → answer → 双向 ice。
 * 两条 DataChannel 由被控端创建并在 answer 侧经 `ondatachannel` 收到：
 *   control（ordered + 可靠）/ pointer（maxRetransmits:0 + 可丢）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import type {
  RemoteDesktopControlEvent,
  RemoteDesktopPointerEvent,
  RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";
import { usePlatform } from "@/hooks/usePlatform.js";
import { logger } from "@/logger.js";
import { applyPreferredRemoteDesktopVideoCodecs } from "./codecPreferences.js";
import { attachRemoteDesktopInput, type RemoteDesktopInputCapture } from "./InputCapture.js";
import {
  deriveRemoteDesktopSessionStats,
  EMPTY_REMOTE_DESKTOP_SESSION_STATS,
  type RemoteDesktopFpsBaseline,
  type RemoteDesktopSessionStats,
} from "./sessionStats.js";
import {
  INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE,
  reduceRemoteDesktopSession,
  type RemoteDesktopSessionEffect,
  type RemoteDesktopSessionEvent,
  type RemoteDesktopSessionViewState,
} from "./sessionReducer.js";

/** 统计轮询间隔（spec UI 规则：状态条延迟/帧率用 getStats 3s 轮询）。 */
const REMOTE_DESKTOP_STATS_INTERVAL_MS = 3000;

export interface UseRemoteDesktopSessionResult {
  view: RemoteDesktopSessionViewState;
  stats: RemoteDesktopSessionStats;
  peerConnectionState: RTCPeerConnectionState | null;
  /** 视频面容器：键盘焦点与输入采集挂点。 */
  containerRef: React.RefObject<HTMLDivElement | null>;
  videoRef: React.RefObject<HTMLVideoElement | null>;
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
  const videoRef = useRef<HTMLVideoElement | null>(null);
  // 视图状态放在 ref + state 双持：reducer updater 不能被 React 重复调用（StrictMode 下
  // 会产生重复 answer/ICE 信令），因此 effect 在事件处理点上精确执行一次。
  const viewRef = useRef<RemoteDesktopSessionViewState>(INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE);
  const [view, setView] = useState<RemoteDesktopSessionViewState>(
    INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE,
  );
  const [stats, setStats] = useState<RemoteDesktopSessionStats>(EMPTY_REMOTE_DESKTOP_SESSION_STATS);
  const [peerConnectionState, setPeerConnectionState] = useState<RTCPeerConnectionState | null>(
    null,
  );

  const pcRef = useRef<RTCPeerConnection | null>(null);
  const controlChannelRef = useRef<RTCDataChannel | null>(null);
  const pointerChannelRef = useRef<RTCDataChannel | null>(null);
  const pendingCandidatesRef = useRef<RTCIceCandidateInit[]>([]);
  const answerInFlightRef = useRef(false);

  const sendSignal = useCallback(
    (message: RemoteDesktopSignalMessage) => {
      platform.rdSendSignal(message);
    },
    [platform],
  );

  const addIceCandidate = useCallback(async (candidate: RTCIceCandidateInit): Promise<void> => {
    const pc = pcRef.current;
    if (!pc) {
      return;
    }
    if (!pc.remoteDescription) {
      // offer 之前的 trickle 候选：先排队，setRemoteDescription 后按序补加。
      pendingCandidatesRef.current.push(candidate);
      return;
    }
    try {
      await pc.addIceCandidate(candidate);
    } catch (error) {
      logger.warn(
        "[remote-desktop] addIceCandidate 失败:",
        error instanceof Error ? error.message : String(error),
      );
    }
  }, []);

  const applyEffect = useCallback(
    (effect: RemoteDesktopSessionEffect): void => {
      switch (effect.kind) {
        case "signal":
          sendSignal(effect.message);
          return;
        case "end-session":
          void platform.rdEndSession(effect.reason).catch(() => undefined);
          return;
        case "add-ice-candidate":
          void addIceCandidate(effect.candidate);
          return;
        case "apply-offer": {
          const pc = pcRef.current;
          if (!pc || answerInFlightRef.current) {
            return;
          }
          answerInFlightRef.current = true;
          void (async () => {
            try {
              await pc.setRemoteDescription({ type: "offer", sdp: effect.sdp });
              const queued = pendingCandidatesRef.current;
              pendingCandidatesRef.current = [];
              for (const candidate of queued) {
                try {
                  await pc.addIceCandidate(candidate);
                } catch {
                  // 单个候选失败不阻断协商（其余候选仍可打通）。
                }
              }
              const answer = await pc.createAnswer();
              await pc.setLocalDescription(answer);
              const sdp = pc.localDescription?.sdp ?? answer.sdp;
              if (!sdp) {
                throw new Error("answer sdp missing");
              }
              sendSignal({ type: "answer", sdp });
            } catch (error) {
              logger.warn(
                "[remote-desktop] 处理 offer 失败:",
                error instanceof Error ? error.message : String(error),
              );
              dispatchRef.current({ type: "media", status: "failed" });
            } finally {
              answerInFlightRef.current = false;
            }
          })();
          return;
        }
      }
    },
    [addIceCandidate, platform, sendSignal],
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

  // ---- 会话生命周期：建 PeerConnection → 订阅 main 事件 → 发起会话 ----
  useEffect(() => {
    let disposed = false;
    const pc = new RTCPeerConnection({ iceServers: [] });
    applyPreferredRemoteDesktopVideoCodecs(pc);
    pcRef.current = pc;

    pc.ondatachannel = (event) => {
      if (event.channel.label === "control") {
        controlChannelRef.current = event.channel;
        return;
      }
      if (event.channel.label === "pointer") {
        pointerChannelRef.current = event.channel;
      }
    };
    pc.ontrack = (event) => {
      const video = videoRef.current;
      const stream = event.streams[0];
      if (!video || !stream || video.srcObject === stream) {
        return;
      }
      video.srcObject = stream;
      void video.play().catch((error: unknown) => {
        logger.warn(
          "[remote-desktop] 视频自动播放失败:",
          error instanceof Error ? error.message : String(error),
        );
      });
    };
    pc.onicecandidate = (event) => {
      if (!event.candidate) {
        return;
      }
      sendSignal({
        type: "ice",
        candidate: event.candidate.candidate,
        sdpMid: event.candidate.sdpMid ?? null,
        sdpMLineIndex: event.candidate.sdpMLineIndex ?? null,
      });
    };
    pc.onconnectionstatechange = () => {
      setPeerConnectionState(pc.connectionState);
      if (pc.connectionState === "connected") {
        dispatchRef.current({ type: "media", status: "live" });
        return;
      }
      if (pc.connectionState === "failed") {
        // ICE 超时等建立失败：终止会话并归因（不留半开状态）。
        dispatchRef.current({ type: "media", status: "failed" });
      }
    };

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
        }
      } catch (error) {
        if (disposed) {
          return;
        }
        logger.warn(
          "[remote-desktop] 发起会话失败:",
          error instanceof Error ? error.message : String(error),
        );
        dispatchRef.current({
          type: "controller",
          event: { kind: "state", state: "closed", error: "internal" },
        });
      }
    })();

    return () => {
      disposed = true;
      unsubscribe();
      try {
        controlChannelRef.current?.close();
        pointerChannelRef.current?.close();
      } catch {
        // 通道随 PeerConnection 关闭，失败可忽略。
      }
      try {
        pc.close();
      } catch {
        // 重复关闭允许幂等失败。
      }
      pcRef.current = null;
      controlChannelRef.current = null;
      pointerChannelRef.current = null;
      pendingCandidatesRef.current = [];
      answerInFlightRef.current = false;
      setPeerConnectionState(null);
      const video = videoRef.current;
      if (video) {
        video.srcObject = null;
      }
      // 离开页面即收口会话：main 侧幂等（对端已 bye / 通道已断时无副作用）。
      void platform.rdEndSession("user-end").catch(() => undefined);
    };
  }, [platform, sendSignal, serverId]);

  // ---- 输入采集：容器焦点 + 视频面 pointer/keyboard → DataChannel ----
  useEffect(() => {
    const container = containerRef.current;
    const video = videoRef.current;
    if (!container || !video) {
      return;
    }
    const sendControl = (event: RemoteDesktopControlEvent): void => {
      const channel = controlChannelRef.current;
      if (!channel || channel.readyState !== "open") {
        return;
      }
      try {
        channel.send(JSON.stringify(event));
      } catch (error) {
        logger.warn(
          "[remote-desktop] control 通道发送失败:",
          error instanceof Error ? error.message : String(error),
        );
      }
    };
    const sendPointer = (event: RemoteDesktopPointerEvent): void => {
      const channel = pointerChannelRef.current;
      if (!channel || channel.readyState !== "open") {
        return;
      }
      try {
        channel.send(JSON.stringify(event));
      } catch {
        // pointer 通道允许丢弃：发送失败不重试、不打断输入流。
      }
    };
    const capture: RemoteDesktopInputCapture = attachRemoteDesktopInput({
      container,
      video,
      sink: { sendControl, sendPointer },
    });
    // 进入会话面即聚焦：键盘事件无需先点击画面。
    container.focus({ preventScroll: true });
    return () => capture.detach();
  }, []);

  // ---- 状态条统计：getStats 3s 轮询（fps / RTT / 分辨率），读数推导在 sessionStats.ts ----
  useEffect(() => {
    let baseline: RemoteDesktopFpsBaseline | null = null;
    const timer = window.setInterval(() => {
      const pc = pcRef.current;
      if (!pc || pc.connectionState !== "connected") {
        return;
      }
      void (async () => {
        try {
          const sample = deriveRemoteDesktopSessionStats(await pc.getStats(), baseline);
          baseline = sample.baseline;
          setStats(sample.stats);
        } catch (error) {
          logger.warn(
            "[remote-desktop] getStats 失败:",
            error instanceof Error ? error.message : String(error),
          );
        }
      })();
    }, REMOTE_DESKTOP_STATS_INTERVAL_MS);
    return () => window.clearInterval(timer);
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
    peerConnectionState,
    containerRef,
    videoRef,
    selectMonitor,
    disconnect,
  };
}
