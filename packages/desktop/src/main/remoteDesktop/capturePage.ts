/**
 * 被控端隐藏采集页（自包含 HTML，运行时写到临时文件后 loadFile；不引入打包资源）。
 *
 * 与 main 的桥（本页无 preload，桥法与 windowsContent.ts 同源）：
 * - main → 页面：`window.__rdCapture.deliver(message)`（RemoteDesktopSignalMessage，main 已校验）
 * - 页面 → main：`window.__rdCapture.takeOutbound()` 轮询，返回一批
 *   `{kind:"signal", message}` / `{kind:"input", channel, payload}` / `{kind:"diag", message}`；
 *   有消息立即返回，空闲最多 250ms 返回空数组——main 侧循环因此不与悬挂 promise 赛跑。
 *
 * 页面只做 WebRTC 与媒体，不做坐标换算（换算在 main，见 inputConversion.ts）；信令只处理
 * select-monitor / answer / ice / bye（hello / monitors 由 main 处理，因为显示器枚举用
 * desktopCapturer + screen 模块）。
 *
 * 采集源由 main 的 `setDisplayMediaRequestHandler` 按选中显示器给出（handler 只认本窗口），
 * 页面侧 `getDisplayMedia` 不需要用户手势，也不弹选择器。
 *
 * 画质档位（spec 产品规则 8）：信令 union（T1 冻结面）没有质量消息，一期在采集侧固定
 * `contentHint = "detail"`（清晰优先）+ sender 级 `maintain-framerate` / 8Mbps 上限，
 * 采集约束与编解码偏好见 spec「质量与延迟参数」；「流畅优先」档位切换需先扩协议。
 */
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const CAPTURE_SCRIPT = `
window.__rdCapture = (() => {
  const outbound = [];
  let notify = null;
  const enqueue = (item) => {
    outbound.push(item);
    if (notify) { const n = notify; notify = null; n(); }
  };
  const takeOutbound = () => {
    if (outbound.length > 0) return Promise.resolve(outbound.splice(0, outbound.length));
    // 空闲也要在有界时间内返回空数组：main 侧的轮询循环只依赖 closed 标志退出，
    // 不需要与一个可能永不 resolve 的 executeJavaScript promise 赛跑。
    return new Promise((resolve) => {
      const timer = setTimeout(() => { if (notify === onMessage) notify = null; resolve([]); }, 250);
      const onMessage = () => { clearTimeout(timer); resolve(outbound.splice(0, outbound.length)); };
      notify = onMessage;
    });
  };

  let pc = null;
  let stream = null;
  let videoTrack = null;
  let pendingCandidates = [];
  let wiredChannels = new Set();

  const diag = (message) => enqueue({ kind: "diag", message: String(message) });

  const waitIceGathering = (peer) => new Promise((resolve) => {
    if (peer.iceGatheringState === "complete") { resolve(); return; }
    const done = () => resolve();
    peer.addEventListener("icegatheringstatechange", () => {
      if (peer.iceGatheringState === "complete") done();
    });
    setTimeout(done, 5000);
  });

  const wireChannel = (channel, name) => {
    if (!channel || wiredChannels.has(name)) return;
    wiredChannels.add(name);
    channel.onmessage = (event) => {
      let payload = null;
      try { payload = JSON.parse(String(event.data)); } catch (error) { diag("bad dc payload: " + error); return; }
      if (!payload || typeof payload !== "object" || typeof payload.t !== "string") return;
      enqueue({ kind: "input", channel: name, payload });
    };
  };

  const captureStream = async () => {
    // 质量与延迟参数（spec「质量与延迟参数」）：60fps 是跟手感的主力（帧周期 33→16ms），
    // 编码跟不上时 maintain-framerate 先降分辨率不降帧率；宽高 ideal 1080p 把 >1080p 源的
    // 编码输入封顶（1080p 源无变化）；displaySurface 与 main 侧选源（types:["screen"]）同口径。
    const fresh = await navigator.mediaDevices.getDisplayMedia({
      video: {
        frameRate: { ideal: 60, max: 60 },
        width: { ideal: 1920 },
        height: { ideal: 1080 },
        displaySurface: "monitor",
      },
      audio: false,
    });
    const track = fresh.getVideoTracks()[0];
    if (track && "contentHint" in track) track.contentHint = "detail";
    return { fresh, track };
  };

  // 与主控 codecPreferences.ts 同款：优先 H.264（Windows 硬编）、VP8 兜底（spec 迁移兼容性）。
  // 页面是自包含内联脚本、无模块导入，这里镜像该排序；失败回退默认协商顺序。
  const applyPreferredCodecs = (peer) => {
    try {
      const getCapabilities = RTCPeerConnection.getCapabilities;
      if (typeof getCapabilities !== "function") return;
      const codecs = getCapabilities("video")?.codecs ?? [];
      if (codecs.length === 0 || typeof peer.setCodecPreferences !== "function") return;
      const h264 = codecs.filter((codec) => codec.mimeType.toLowerCase() === "video/h264");
      const vp8 = codecs.filter((codec) => codec.mimeType.toLowerCase() === "video/vp8");
      if (h264.length === 0 && vp8.length === 0) return;
      const preferred = new Set([...h264, ...vp8]);
      peer.setCodecPreferences([...h264, ...vp8, ...codecs.filter((codec) => !preferred.has(codec))]);
    } catch (error) {
      diag("setCodecPreferences fallback: " + error);
    }
  };

  // 编码参数（spec「质量与延迟参数」）：带宽紧张先降分辨率不降帧率；8Mbps 上限给动态场景
  // 留足码率（此前完全没设，靠浏览器默认）。setParameters 失败只记诊断，回退默认协商。
  const applyVideoSenderParameters = async (peer) => {
    const sender = peer.getSenders().find((candidate) => candidate.track && candidate.track.kind === "video");
    if (!sender) return;
    try {
      const params = sender.getParameters();
      // encodings 长度必须与 getParameters 返回的一致（否则 InvalidModificationError）：
      // Chromium 在 addTrack 后通常已带一条，这里只往既有条目上补 maxBitrate。
      const encodings = params.encodings && params.encodings.length > 0 ? params.encodings : [{}];
      encodings[0].maxBitrate = 8_000_000;
      // 低延迟编码请求（Chrome 支持则生效，不认识该字典成员的实现按 WebIDL 规则忽略）。
      encodings[0].latencyHint = 0.02;
      params.encodings = encodings;
      params.degradationPreference = "maintain-framerate";
      await sender.setParameters(params);
    } catch (error) {
      diag("setParameters fallback: " + error);
    }
  };

  const ensurePeer = async () => {
    if (pc) return pc;
    const captured = await captureStream();
    stream = captured.fresh;
    videoTrack = captured.track;
    pc = new RTCPeerConnection({ iceServers: [] });
    applyPreferredCodecs(pc);
    if (videoTrack) pc.addTrack(videoTrack, stream);
    void applyVideoSenderParameters(pc);
    wireChannel(pc.createDataChannel("control", { ordered: true }), "control");
    wireChannel(pc.createDataChannel("pointer", { maxRetransmits: 0 }), "pointer");
    pc.onicecandidate = (event) => {
      if (!event.candidate) return;
      enqueue({
        kind: "signal",
        message: {
          type: "ice",
          candidate: event.candidate.candidate,
          sdpMid: event.candidate.sdpMid ?? null,
          sdpMLineIndex: event.candidate.sdpMLineIndex ?? null,
        },
      });
    };
    pc.onconnectionstatechange = () => {
      if (pc && (pc.connectionState === "failed" || pc.connectionState === "disconnected")) {
        diag("peer connection " + pc.connectionState);
      }
    };
    return pc;
  };

  const fail = (error) => {
    diag(error);
    enqueue({ kind: "signal", message: { type: "error", code: "internal", message: String(error) } });
  };

  const selectMonitor = async (displayId) => {
    try {
      if (pc) {
        // 会话中切换显示器：replaceTrack，不重发 offer（spec 验收：换轨不重建会话）。
        const captured = await captureStream();
        const sender = pc.getSenders().find((candidate) => candidate.track && candidate.track.kind === "video");
        if (!sender) throw new Error("video sender missing");
        await sender.replaceTrack(captured.track);
        if (videoTrack) videoTrack.stop();
        videoTrack = captured.track;
        stream = captured.fresh;
        diag("monitor switched: " + displayId);
        return;
      }
      const peer = await ensurePeer();
      const offer = await peer.createOffer();
      await peer.setLocalDescription(offer);
      await waitIceGathering(peer);
      enqueue({ kind: "signal", message: { type: "offer", sdp: peer.localDescription.sdp } });
    } catch (error) {
      fail(error);
    }
  };

  const flushCandidates = async () => {
    const queued = pendingCandidates;
    pendingCandidates = [];
    for (const candidate of queued) {
      try {
        await pc.addIceCandidate({ candidate: candidate.candidate, sdpMid: candidate.sdpMid, sdpMLineIndex: candidate.sdpMLineIndex });
      } catch (error) {
        diag("addIceCandidate failed: " + error);
      }
    }
  };

  const closePeer = () => {
    try { if (pc) pc.close(); } catch (error) { diag("close pc failed: " + error); }
    try { if (videoTrack) videoTrack.stop(); } catch { /* 幂等 */ }
    try { if (stream) stream.getTracks().forEach((track) => track.stop()); } catch { /* 幂等 */ }
    pc = null; stream = null; videoTrack = null; pendingCandidates = []; wiredChannels = new Set();
  };

  const deliver = (message) => {
    if (!message || typeof message.type !== "string") return;
    if (message.type === "select-monitor") { void selectMonitor(message.displayId); return; }
    if (message.type === "answer") {
      void (async () => {
        try {
          if (!pc) return;
          await pc.setRemoteDescription({ type: "answer", sdp: message.sdp });
          await flushCandidates();
        } catch (error) { fail(error); }
      })();
      return;
    }
    if (message.type === "ice") {
      void (async () => {
        try {
          if (!pc || !pc.remoteDescription) { pendingCandidates.push(message); return; }
          await pc.addIceCandidate({ candidate: message.candidate, sdpMid: message.sdpMid, sdpMLineIndex: message.sdpMLineIndex });
        } catch (error) { diag("addIceCandidate failed: " + error); }
      })();
      return;
    }
    if (message.type === "bye") { closePeer(); return; }
  };

  return { takeOutbound, deliver, closePeer };
})();
`;

function capturePageHtml(): string {
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'">
  <title>Mode Remote Desktop Capture</title>
</head>
<body>
  <script>${CAPTURE_SCRIPT}</script>
</body>
</html>`;
}

/** 写到独立临时目录（加载后立即删除；与 electronBrowserWebmRecorder 的临时文档先例一致）。 */
export async function writeRemoteDesktopCapturePage(): Promise<{
  path: string;
  cleanup: () => Promise<void>;
}> {
  const dir = await mkdtemp(join(tmpdir(), "mode-remote-desktop-"));
  const path = join(dir, "capture.html");
  await writeFile(path, capturePageHtml(), { encoding: "utf8", mode: 0o600 });
  return {
    path,
    cleanup: async () => {
      await rm(dir, { recursive: true, force: true }).catch(() => undefined);
    },
  };
}
