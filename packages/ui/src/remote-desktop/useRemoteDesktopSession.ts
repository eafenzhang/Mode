/* oxlint-disable eslint(max-lines) -- 会话生命周期/自适应码率/改档与动态降档编排集中一处：
   拆分会让 bye+重连、媒体抑制位、判重键的收口路径散化到多文件，漏一条就是半开会话。 */
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
import {
  DEFAULT_REMOTE_DESKTOP_QUALITY,
  REMOTE_DESKTOP_QUALITY_PRESETS,
  type RemoteDesktopCodec,
  type RemoteDesktopQuality,
  type RemoteDesktopControlEvent,
  type RemoteDesktopPointerEvent,
  type RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";
import { usePlatform } from "@/hooks/usePlatform.js";
import { logger } from "@/logger.js";
import { attachRemoteDesktopInput, type RemoteDesktopInputCapture } from "./InputCapture.js";
import {
  createRemoteDesktopClipboardApplier,
  type RemoteDesktopClipboardApplier,
} from "./clipboardSync.js";
import { createNativeMediaSession, probeHevcDecodeSupport, type NativeMediaSession } from "./nativeMediaSession.js";
import {
  applyCodecProbe,
  INITIAL_CODEC_FALLBACK_STATE,
  stepCodecDowngrade,
  type CodecFallbackState,
} from "./codecFallback.js";
import {
  initialAdaptiveBitrateState,
  nextLowerQuality,
  stepAdaptiveBitrate,
  stepQualityDownshift,
  INITIAL_QUALITY_DOWNSHIFT_STATE,
  type QualityDownshiftState,
} from "./adaptiveBitrate.js";
import {
  createSessionReconnectController,
  type SessionReconnectController,
} from "./sessionReconnectController.js";
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

/**
 * 剪贴板轮询间隔（P1.1b）：与被控 helper 侧同量级——文本同步是便利功能，
 * 再快也只是把 OpenClipboard 争用变成常态（读的是本机剪贴板，代价是 IPC + 权限检查）。
 */
const REMOTE_DESKTOP_CLIPBOARD_POLL_MS = 700;

/** 媒体票重签单次预算（信令存活时应远快于此；超时按本轮失败走下一档退避）。 */
const MEDIA_TICKET_REQUEST_TIMEOUT_MS = 2_500;

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
  /**
   * 会话面操作栏改档（spec: native-media「画质档位」2026-10-13）：fps/maxWidth 会话
   * 建立时定型 → bye 结束旧会话后按 P2-1 重连编排以新档重建（被控 90s 窗口同设备
   * 免二次同意；busy/瞬时错误按既有退避自动重试）。调用方须先更新 quality props。
   */
  switchQuality: (next: RemoteDesktopQuality) => Promise<void>;
  /**
   * 剪贴板同步开关状态（P1.1a，spec 产品规则 7）：默认开。
   * 关闭只停同步——本端不再把下行文本落进剪贴板，并同时上行 `clip-mode` 让被控中继
   * 停止采纳；画面与输入不受影响。
   */
  clipboardEnabled: boolean;
  setClipboardEnabled: (enabled: boolean) => void;
}

export function useRemoteDesktopSession(options: {
  serverId: string;
  /** 画质档位（P1-3）：随会话请求下发给被控；缺省 = 被控默认档。 */
  quality?: RemoteDesktopQuality;
  /** 档位变化回写（手动改档与自动降档共用一条路径——页级记忆）。 */
  onQualityChanged?: (quality: RemoteDesktopQuality) => void;
}): UseRemoteDesktopSessionResult {
  const { serverId, quality } = options;
  const platform = usePlatform();
  const qualityPreset = REMOTE_DESKTOP_QUALITY_PRESETS[quality ?? DEFAULT_REMOTE_DESKTOP_QUALITY];
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
  // → 下行 {t:"set-bitrate"}（中继 → helper → ICodecAPI MeanBitRate）。两档取自本会话
  // 画质档位预设（P1-3）：高档 = 预设码率、低档 = 预设低码率，拥塞时不越级到其它档。
  const adaptiveRef = useRef(
    initialAdaptiveBitrateState(qualityPreset.bitrateBps, qualityPreset.minBitrateBps),
  );
  const droppedSinceAdaptiveSample = useRef(0);
  // 断线重连（P2-1）：媒体票请求的等待者（media-ticket 信令回复唤醒）；
  // 会话级重连期间的媒体失败抑制位（旧媒体面静默收口，不触发失败归因）。
  const pendingMediaTicketRef = useRef<((ticket: string | null) => void) | null>(null);
  const suppressMediaFailureRef = useRef(false);
  // 画质档位档位 ref（P1-3 入口迁移 2026-10-13）：首连/重连闭包读最新档。
  // 每次渲染同步 props——改档时页级状态先行/后行都收敛到同一值；**quality 不进挂载
  // effect 依赖**：改档走「bye + 重连」显式路径，不重跑事件订阅（重订阅会丢 main 的
  // connected 推送，握手停在等 monitors）。
  const qualityRef = useRef(quality);
  qualityRef.current = quality;
  // options 的最新快照（onQualityChanged 等回调经 ref 消费，不重建闭包/不进 effect 依赖）。
  const optionsRef = useRef(options);
  optionsRef.current = options;
  // 会话级重连编排器（P2-1）：挂载 effect 内创建，switchQuality 经 ref 调用。
  const reconnectControllerRef = useRef<SessionReconnectController | null>(null);
  // 动态降档判定状态（spec 画质档位·动态降档）：改档重建（手动/自动）即复位。
  const qualityDownshiftRef = useRef<QualityDownshiftState>(INITIAL_QUALITY_DOWNSHIFT_STATE);
  // 编码 codec 偏好（HEVC 选型 2026-10-13）：每挂载预检一次本机解码能力并缓存——
  // 首连/重连/改档共用；unsupported → h264。预检失败（异常）按不支持处理。
  // 解码真失败时按 codecFallback 单向降为 h264 并重建一次（见 open-media 的 onDecoderFailed）。
  const codecFallbackRef = useRef<CodecFallbackState>(INITIAL_CODEC_FALLBACK_STATE);
  const resolveCodecPref = useCallback(async (): Promise<RemoteDesktopCodec> => {
    const state = codecFallbackRef.current;
    if (state.codec !== null) {
      return state.codec;
    }
    const hevcSupported = await probeHevcDecodeSupport();
    codecFallbackRef.current = applyCodecProbe(state, hevcSupported);
    return codecFallbackRef.current.codec ?? "h264";
  }, []);

  const sendSignal = useCallback(
    (message: RemoteDesktopSignalMessage) => {
      platform.rdSendSignal(message);
    },
    [platform],
  );

  /** 媒体 WS 瞬断重连（P2-1）：经信令通道要一张新媒体票，拼出新的媒体地址。 */
  const requestFreshMediaUrl = useCallback(async (): Promise<string | null> => {
    const base = mediaUrlRef.current;
    if (!base) {
      return null;
    }
    const ticket = await new Promise<string | null>((resolve) => {
      let settled = false;
      const timer = window.setTimeout(() => {
        if (!settled) {
          settled = true;
          pendingMediaTicketRef.current = null;
          resolve(null);
        }
      }, MEDIA_TICKET_REQUEST_TIMEOUT_MS);
      pendingMediaTicketRef.current = (value) => {
        if (settled) {
          return;
        }
        settled = true;
        window.clearTimeout(timer);
        resolve(value);
      };
      platform.rdSendSignal({ type: "request-media-ticket" });
    });
    if (!ticket) {
      return null;
    }
    try {
      const url = new URL(base);
      url.searchParams.set("mediaTicket", ticket);
      return url.toString();
    } catch {
      return null;
    }
  }, [platform]);

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
            // 新媒体面即将建立：解除会话级重连的失败抑制（此后该会话的真实失败照常归因）。
            suppressMediaFailureRef.current = false;
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
              // 首帧看门狗（spec「媒体面可观测性与首帧约束」）：媒体 WS 已开但窗口内一帧没画
              // → 页面切「已连接，等待对端首帧…」（不再无提示纯黑）；首个已画帧即撤除。
              // 「一帧都没画」的 warn 与计数在 createNativeMediaSession 内部落盘，此处只切 UI 态。
              onFirstFrame: () => {
                dispatchRef.current({ type: "first-frame", seen: true });
              },
              onNoFirstFrame: () => {
                dispatchRef.current({ type: "first-frame", seen: false });
              },
              // HEVC 解码失败降级（spec「媒体面可观测性与首帧约束」）：预检通过 ≠ 真能解
              //（profile/位深/驱动差异），失败时把偏好降为 H.264 并放行一次整会话重建，
              // 而不是直接把会话判死。降级只发生一次，之后走既有的媒体失败归因。
              onDecoderFailed: () => {
                const step = stepCodecDowngrade(codecFallbackRef.current);
                codecFallbackRef.current = step.state;
                if (!step.downgrade) {
                  return;
                }
                logger.warn("[remote-desktop] HEVC 解码失败，降级 H.264 并重建会话");
                // 旧媒体面随即收口（decoder error → teardown → onTransportClosed）：抑制
                // 这次失败归因，交给重连编排用新偏好重建整会话，别让会话被失败路径关掉。
                suppressMediaFailureRef.current = true;
                reconnectControllerRef.current?.begin();
              },
              onFrameDropped: () => {
                // 解码队列门控丢弃：累计到下一次自适应采样（与 RTT 同拍判定），
                // 同时计入状态条读数（丢帧 > 0 时状态条显式提示）。
                droppedSinceAdaptiveSample.current += 1;
                nativeStats.current.onFrameDropped();
              },
              onBytesReceived: (bytes) => {
                // 码率读数：近一拍收到的密文字节数，3s 采样折算 kbps。
                nativeStats.current.onBytesReceived(bytes);
              },
              onDecodeQueue: (size) => {
                // 解码队列长度：最近一次被接受帧的即时读数（解释丢帧与延迟堆积）。
                nativeStats.current.onDecodeQueue(size);
              },
              onFrameAgeMs: (ageMs) => {
                // 帧龄读数（glass-to-glass 近似）：随下一拍 3s 采样进状态条（取窗口中位数）。
                nativeStats.current.onFrameAge(ageMs);
              },
              // 媒体 WS 瞬断重连（P2-1）：要新票 → 同一会话续流；重连期间状态条提示「正在恢复」。
              onRequestMediaUrl: () => requestFreshMediaUrl(),
              onReconnecting: (reconnecting) => {
                dispatchRef.current({
                  type: "media",
                  status: reconnecting ? "reconnecting" : "live",
                });
              },
              // 传输断开（重连退避耗尽 / 无重连能力 / 解码器错误）：按媒体失败收口，不留半开状态。
              // 会话级重连进行中旧媒体面被主动停止：抑制位吞掉这次通知（恢复走新会话）。
              onTransportClosed: () => {
                if (suppressMediaFailureRef.current) {
                  return;
                }
                dispatchRef.current({ type: "media", status: "failed" });
              },
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
    [platform, requestFreshMediaUrl, sendSignal],
  );
  const applyEffectRef = useRef(applyEffect);
  applyEffectRef.current = applyEffect;

  const dispatch = useCallback((event: RemoteDesktopSessionEvent): void => {
    // 冻结次数在唯一的状态迁移入口计数（freeze 重复到达幂等，只有未冻结→冻结记一次）。
    if (event.type === "freeze" && !viewRef.current.frozen) {
      nativeStats.current.onFrozen();
    }
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

    // 会话级重连（P2-1）：编排（退避/重试分类）在纯模块 sessionReconnectController，
    // 本处只接三个落点——静默停旧媒体面、新 mediaUrl 接力、失败收口。
    const reconnectController = createSessionReconnectController({
      // 画质档位（P1-3）与 codec 偏好均读最新值（改档/能力缓存后重建也走这条编排）。
      startSession: async () =>
        platform.rdStartSession(serverId, qualityRef.current, await resolveCodecPref()),
      onBegin: () => {
        suppressMediaFailureRef.current = true;
        sessionRef.current?.stop();
        sessionRef.current = null;
        dispatchRef.current({ type: "media", status: "reconnecting" });
      },
      onRecovered: (mediaUrl) => {
        // 后续 connecting→connected→hello→monitors→media-open 由事件流驱动；
        // 新媒体面建立（open-media effect）时解除失败抑制并恢复画面。
        mediaUrlRef.current = mediaUrl;
      },
      onGiveUp: (error) => {
        suppressMediaFailureRef.current = false;
        dispatchRef.current({
          type: "controller",
          event: { kind: "state", state: "closed", error },
        });
        // 重连/改档退避耗尽：媒体态从「正在恢复」收敛为失败——否则会话面把
        // closed && reconnecting 当瞬态遮蔽，真实失败的关闭层永远不显示。
        dispatchRef.current({ type: "media", status: "failed" });
      },
    });

    reconnectControllerRef.current = reconnectController;

    const unsubscribe = platform.onRemoteDesktopSessionEvent((event) => {
      // 媒体票回复（P2-1，媒体 WS 瞬断重连）：只有重连路径在等它，不进 reducer。
      if (event.kind === "signal" && event.message.type === "media-ticket") {
        const resolve = pendingMediaTicketRef.current;
        pendingMediaTicketRef.current = null;
        resolve?.(event.message.mediaTicket);
        return;
      }
      // 传输断开（非本地结束、非对端 bye）：会话级重连退避，而不是立即收口；
      // closed 事件不下发 reducer——会话面保持挂载，重连成功走 connecting→connected 正常迁移。
      if (event.kind === "state" && event.state === "closed" && event.retryable) {
        reconnectController.begin();
        return;
      }
      dispatchRef.current({ type: "controller", event });
    });

    void (async () => {
      try {
        // 画质档位 + codec 偏好随会话请求下发（P1-3 / HEVC 选型）：被控据此定
        // 采集/编码参数与编码 codec（缺省/不支持 = h264）。
        const codec = await resolveCodecPref();
        const result = await platform.rdStartSession(serverId, qualityRef.current, codec);
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
      reconnectController.dispose();
      reconnectControllerRef.current = null;
      unsubscribe();
      // 先断媒体面再收口：会话面卸载即停止解密/绘制，stop 幂等（内部 teardown 只通知一次）。
      sessionRef.current?.stop();
      sessionRef.current = null;
      mediaUrlRef.current = null;
      pendingMediaTicketRef.current = null;
      // 离开页面即收口会话：main 侧幂等（对端已 bye / 通道已断时无副作用）。
      // 已收口的会话（对端 bye / 媒体失败 / 本地断开）不重复归因：卸载兜底只服务仍在进行的会话，
      // 避免给已按 error/remote 归因结束的会话再补一次 user-end（台账 Task 10 项）。
      // 重连进行中（phase 仍为 connected）离开时同样收口——不留无人恢复的半开会话。
      if (viewRef.current.phase !== "closed") {
        void platform.rdEndSession("user-end").catch(() => undefined);
      }
    };
    // quality/codec 经 ref 与 resolveCodecPref 消费：改档/预检不触发本 effect 重跑。
  }, [platform, serverId, resolveCodecPref]);

  // ---- media WS 下行 JSON 控制事件（Task 9 胶水派发到 window，与信令面解耦）----
  useEffect(() => {
    const onControl = (event: Event) => {
      const detail = (
        event as CustomEvent<{ t?: string; ts?: number; reason?: string; hostTs?: number }>
      ).detail;
      // 剪贴板同步（P1.1a）：下行 `{t:"clip"}` 是否落到本机剪贴板由 applier 判定
      // （回环抑制与开关都在门里，本处只转发，不做第二次判断）。
      clipboardApplierRef.current?.onControlMessage(detail);
      if (detail?.t === "pong" && typeof detail.ts === "number") {
        // RTT = 收到时刻 - ping 时间戳；异步写入 collector，随下一拍 3s 采样进入状态条。
        const rttMs = Date.now() - detail.ts;
        nativeStats.current.onPong(rttMs);
        // 时钟偏移标定（帧龄用）：offset ≈ hostTs − ts − RTT/2（对称路径假设；LAN 误差数 ms）。
        if (typeof detail.hostTs === "number") {
          sessionRef.current?.setClockOffset(detail.hostTs - detail.ts - rttMs / 2);
        }
        // 自适应码率判定（与 RTT 同拍；丢弃数取自上拍以来累计，采样后清零）。
        const dropped = droppedSinceAdaptiveSample.current;
        const outcome = stepAdaptiveBitrate(
          adaptiveRef.current,
          { rttMs, droppedFrames: dropped },
          Date.now(),
        );
        adaptiveRef.current = outcome.state;
        droppedSinceAdaptiveSample.current = 0;
        if (outcome.nextBps !== null) {
          sessionRef.current?.send({ t: "set-bitrate", bps: outcome.nextBps });
        }
        // 动态降档（spec 画质档位·动态降档 2026-10-13）：低档码率仍持续拥塞 →
        // 自动以更低画质档重建会话（复用改档重建链路；pong 只在媒体存活时到达，
        // 重连/收口期间天然不触发）。switchQuality 经 ref 调用（本 effect deps 为空）。
        const downshift = stepQualityDownshift(
          qualityDownshiftRef.current,
          { rttMs, droppedFrames: dropped },
          Date.now(),
        );
        qualityDownshiftRef.current = downshift.state;
        if (downshift.downshift) {
          const lower = qualityRef.current ? nextLowerQuality(qualityRef.current) : null;
          if (lower) {
            logger.info(`[remote-desktop] 持续拥塞自动降档 → ${lower}`);
            void switchQualityRef.current(lower);
          }
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

  const switchQuality = useCallback(
    async (next: RemoteDesktopQuality): Promise<void> => {
      if (qualityRef.current === next) {
        return;
      }
      qualityRef.current = next;
      // 页级记忆回写（手动改档与自动降档共用一条路径）。
      optionsRef.current.onQualityChanged?.(next);
      // 自适应两档改绑新预设（重建会话后高低码率区间随档位走），丢帧窗口与
      // 降档判定状态一并复位（新档从零评估，不在旧拥塞结论上叠加）。
      const preset = REMOTE_DESKTOP_QUALITY_PRESETS[next];
      adaptiveRef.current = initialAdaptiveBitrateState(preset.bitrateBps, preset.minBitrateBps);
      droppedSinceAdaptiveSample.current = 0;
      qualityDownshiftRef.current = INITIAL_QUALITY_DOWNSHIFT_STATE;
      // 先进入重连编排：mediaStatus=reconnecting 把紧随其后的本地 closed 事件遮成
      // 「正在恢复」（避免关闭态闪层），首档尝试在 250ms 后——届时 bye 已过一个 RTT、
      // 被控释放 busy；仍撞 busy 走既有退避（90s 窗口内同设备免二次同意）。
      reconnectControllerRef.current?.begin();
      await platform.rdEndSession("user-end").catch(() => undefined);
    },
    [platform],
  );
  // pong 采样的降档分支经 ref 调用（媒体控制 effect deps 为空，直引会困在首帧闭包）。
  const switchQualityRef = useRef(switchQuality);
  switchQualityRef.current = switchQuality;

  const disconnect = useCallback(() => {
    dispatchRef.current({ type: "local-end" });
  }, []);

  // ---- 剪贴板同步（P1.1a：被控 → 主控）----
  // 门（回环抑制 + 开关）在 applier 内；这里只提供写入器与开关的上行。
  const clipboardApplierRef = useRef<RemoteDesktopClipboardApplier | null>(null);
  if (clipboardApplierRef.current === null) {
    clipboardApplierRef.current = createRemoteDesktopClipboardApplier({
      writeText: async (text) => {
        // 与 UI 其它复制入口同口径：走 navigator.clipboard（Electron 下写剪贴板无需授权）。
        if (typeof navigator === "undefined" || !navigator.clipboard?.writeText) {
          throw new Error("clipboard write unavailable");
        }
        await navigator.clipboard.writeText(text);
      },
      onWriteError: (error) => {
        // 写失败只记日志：剪贴板失败不该影响画面与输入（门已登记该值，不会反复重试）。
        logger.warn("[remote-desktop] 剪贴板写入失败:", error);
      },
    });
  }
  const [clipboardEnabled, setClipboardEnabled] = useState(
    () => clipboardApplierRef.current?.isEnabled() ?? true,
  );
  const setRemoteClipboardEnabled = useCallback((enabled: boolean) => {
    clipboardApplierRef.current?.setEnabled(enabled);
    setClipboardEnabled(enabled);
    // 上行到被控中继（媒体 WS 同通道，P1.1）：开关是会话级的，两侧必须一致。
    // 未连接时 send 是 no-op——此时 renderer 侧的门已生效，下行文本不会被落地。
    sessionRef.current?.send({ t: "clip-mode", enabled });
  }, []);

  // ---- 剪贴板同步（P1.1b：主控 → 被控）----
  // 轮询本机剪贴板，变化即上行 `{t:"clip"}`（被控中继采纳后落到它的剪贴板）。
  // 只在已连接且开关打开时轮询；读到空文本/同值由门判定为「无变化」，不发。
  useEffect(() => {
    if (view.phase !== "connected" || !clipboardEnabled) {
      return;
    }
    let cancelled = false;
    const timer = window.setInterval(() => {
      void platform.rdReadClipboardText().then(
        (text) => {
          if (cancelled) return;
          if (!clipboardApplierRef.current?.observeLocalText(text)) return;
          sessionRef.current?.send({ t: "clip", text });
        },
        (error: unknown) => {
          // 读失败按「本拍无变化」处理：剪贴板争用是常态，不该打断会话。
          logger.debug("[remote-desktop] 剪贴板读取失败:", error);
        },
      );
    }, REMOTE_DESKTOP_CLIPBOARD_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [platform, view.phase, clipboardEnabled]);

  return {
    view,
    stats,
    containerRef,
    canvasRef,
    selectMonitor,
    disconnect,
    switchQuality,
    clipboardEnabled,
    setClipboardEnabled: setRemoteClipboardEnabled,
  };
}
