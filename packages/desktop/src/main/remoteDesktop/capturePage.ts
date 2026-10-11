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
 * `contentHint = "detail"`（清晰优先），码率自适应交给 WebRTC；「流畅优先」需先扩协议。
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
    const fresh = await navigator.mediaDevices.getDisplayMedia({ video: { frameRate: { ideal: 30 } }, audio: false });
    const track = fresh.getVideoTracks()[0];
    if (track && "contentHint" in track) track.contentHint = "detail";
    return { fresh, track };
  };

  const ensurePeer = async () => {
    if (pc) return pc;
    const captured = await captureStream();
    stream = captured.fresh;
    videoTrack = captured.track;
    pc = new RTCPeerConnection({ iceServers: [] });
    if (videoTrack) pc.addTrack(videoTrack, stream);
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
