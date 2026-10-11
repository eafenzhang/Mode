/**
 * 被控端两个浮层窗口的呈现层（同意窗 / 会话指示浮层）。
 *
 * 与窗口生命周期分开（同 windowsCuaOperationIndicatorContent 先例：文案与视觉的变更理由不同）。
 * 自包含页面：data URL + 内联 CSS/JS，不引入新打包资源；页面与 main 的桥是
 * `window.__rd.takeAction()` 长轮询（main 用 executeJavaScript 等待一次用户动作），
 * 因为这两个窗口没有 preload（preload 入口属于打包资源清单）。
 *
 * i18n：键名与 T5 的 `remoteDesktop.*` 对齐，本文件自带 zh-CN 词典并预留 en-US；
 * fa-IR 暂回退 en-US（被控浮层不含 fa 文案，属于两端 UI 文本的已知缺口）。
 */
import type { Locale } from "@mode/shared";

export type RemoteDesktopCopyKey =
  | "remoteDesktop.consent.title"
  | "remoteDesktop.consent.allow"
  | "remoteDesktop.consent.deny"
  | "remoteDesktop.consent.remember"
  | "remoteDesktop.consent.timeout"
  | "remoteDesktop.indicator.text"
  | "remoteDesktop.indicator.end";

/** `{{device}}` 是设备名占位；`{{seconds}}` 是剩余秒数占位。 */
const COPY: Record<"zh-CN" | "en", Record<RemoteDesktopCopyKey, string>> = {
  "zh-CN": {
    "remoteDesktop.consent.title": "{{device}} 请求远程控制本机",
    "remoteDesktop.consent.allow": "允许",
    "remoteDesktop.consent.deny": "拒绝",
    "remoteDesktop.consent.remember": "记住此设备",
    "remoteDesktop.consent.timeout": "{{seconds}} 秒后自动拒绝",
    "remoteDesktop.indicator.text": "{{device}} 正在远程控制本机",
    "remoteDesktop.indicator.end": "结束",
  },
  en: {
    "remoteDesktop.consent.title": "{{device}} wants to control this device",
    "remoteDesktop.consent.allow": "Allow",
    "remoteDesktop.consent.deny": "Deny",
    "remoteDesktop.consent.remember": "Remember this device",
    "remoteDesktop.consent.timeout": "Denied automatically in {{seconds}}s",
    "remoteDesktop.indicator.text": "{{device}} is controlling this device",
    "remoteDesktop.indicator.end": "End",
  },
};

function dictionaryFor(locale: Locale): Record<RemoteDesktopCopyKey, string> {
  return locale === "zh-CN" ? COPY["zh-CN"] : COPY.en;
}

export function remoteDesktopCopy(
  locale: Locale,
  key: RemoteDesktopCopyKey,
  params: { device?: string; seconds?: number } = {},
): string {
  const template = dictionaryFor(locale)[key];
  return template
    .replaceAll("{{device}}", params.device ?? "")
    .replaceAll("{{seconds}}", String(params.seconds ?? ""));
}

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

const CARD_SHADOW_INSET = { top: 6, right: 8, bottom: 12, left: 8 } as const;

export function consentWindowSize(locale: Locale): { width: number; height: number } {
  const cardWidth = locale === "zh-CN" ? 400 : 440;
  return {
    width: cardWidth + CARD_SHADOW_INSET.left + CARD_SHADOW_INSET.right,
    height: CARD_SHADOW_INSET.top + 168 + CARD_SHADOW_INSET.bottom,
  };
}

export function indicatorWindowSize(locale: Locale): { width: number; height: number } {
  const cardWidth = locale === "zh-CN" ? 460 : 520;
  return {
    width: cardWidth + CARD_SHADOW_INSET.left + CARD_SHADOW_INSET.right,
    height: CARD_SHADOW_INSET.top + 40 + CARD_SHADOW_INSET.bottom,
  };
}

/** 两个窗口共用的桥：动作队列 + 长轮询。 */
const ACTION_BRIDGE_SCRIPT = `
  window.__rd = (() => {
    const queue = [];
    let resolver = null;
    return {
      takeAction: () => {
        if (queue.length > 0) return Promise.resolve(queue.shift());
        return new Promise((resolve) => { resolver = resolve; });
      },
      emit: (action) => {
        if (resolver) { const r = resolver; resolver = null; r(action); return; }
        queue.push(action);
      },
    };
  })();
`;

const STATE_STYLE = `
  html { color-scheme: light dark; }
  * { box-sizing: border-box; }
  html, body { width: 100%; height: 100%; margin: 0; overflow: hidden; background: transparent;
    font-family: "Segoe UI Variable", "Segoe UI", sans-serif; }
  body { display: flex; align-items: center; justify-content: center; padding: 6px 8px 12px; }
  .card { width: 100%; color: #1f2328; background: rgba(255, 255, 255, 0.97);
    border: 1px solid rgba(15, 23, 42, 0.14); border-radius: 12px; padding: 14px 16px;
    box-shadow: 0 1px 2px rgba(15, 23, 42, 0.08), 0 8px 16px -8px rgba(15, 23, 42, 0.22); }
  .title { font-size: 14px; font-weight: 600; line-height: 1.35; margin: 0 0 10px; }
  .row { display: flex; align-items: center; gap: 10px; }
  .grow { flex: 1; }
  .hint { font-size: 12px; color: #5b6472; }
  button { font: inherit; font-size: 13px; font-weight: 600; border-radius: 8px; cursor: pointer;
    border: 1px solid rgba(15, 23, 42, 0.18); background: rgba(248, 250, 252, 0.9); color: inherit;
    padding: 6px 14px; }
  button.primary { background: #1f6feb; border-color: #1f6feb; color: #ffffff; }
  label.check { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: #3c4453; user-select: none; }
  .indicator { display: flex; align-items: center; gap: 12px; width: 100%; height: 40px;
    padding: 0 12px 0 16px; }
  .indicator .text { flex: 1; font-size: 13px; font-weight: 600; white-space: nowrap; overflow: hidden;
    text-overflow: ellipsis; }
  @media (prefers-color-scheme: dark) {
    .card { color: #f8fafc; background: rgba(33, 36, 41, 0.97); border-color: rgba(255, 255, 255, 0.14);
      box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25), 0 8px 16px -8px rgba(0, 0, 0, 0.4); }
    .hint, label.check { color: #a8b3c4; }
    button { border-color: rgba(255, 255, 255, 0.18); background: rgba(52, 56, 62, 0.9); }
    button.primary { background: #3b82f6; border-color: #3b82f6; }
  }
`;

function dataUrl(locale: Locale, html: string): string {
  return `data:text/html;base64,${Buffer.from(html, "utf8").toString("base64")}`;
}

/** 同意窗页面：设备名 + 允许/拒绝 + 「记住此设备」+ 倒计时（页面侧只做显示，超时判定在 main）。 */
export function consentWindowDataUrl(
  locale: Locale,
  deviceLabel: string,
  timeoutSeconds: number,
): string {
  const title = escapeHtml(remoteDesktopCopy(locale, "remoteDesktop.consent.title", { device: deviceLabel }));
  const allow = escapeHtml(remoteDesktopCopy(locale, "remoteDesktop.consent.allow"));
  const deny = escapeHtml(remoteDesktopCopy(locale, "remoteDesktop.consent.deny"));
  const remember = escapeHtml(remoteDesktopCopy(locale, "remoteDesktop.consent.remember"));
  // 倒计时模板必须**保留** {{seconds}} 占位（页面每秒替换），因此取原始词条而不是先做替换。
  const timeoutTemplate = escapeHtml(dictionaryFor(locale)["remoteDesktop.consent.timeout"]);
  const html = `<!doctype html>
<html lang="${locale}">
<head>
  <meta charset="utf-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'">
  <style>${STATE_STYLE}</style>
</head>
<body>
  <div class="card">
    <p class="title">${title}</p>
    <div class="row">
      <label class="check"><input id="remember" type="checkbox">${remember}</label>
      <span class="grow"></span>
      <span class="hint" id="countdown"></span>
    </div>
    <div class="row" style="margin-top:14px; justify-content:flex-end;">
      <button id="deny">${deny}</button>
      <button id="allow" class="primary">${allow}</button>
    </div>
  </div>
  <script>
    ${ACTION_BRIDGE_SCRIPT}
    (() => {
      const deadline = Date.now() + ${timeoutSeconds * 1000};
      const countdown = document.getElementById("countdown");
      const template = ${JSON.stringify(timeoutTemplate)};
      const renderCountdown = () => {
        const seconds = Math.max(0, Math.ceil((deadline - Date.now()) / 1000));
        countdown.textContent = template.replace("{{seconds}}", String(seconds));
        return seconds;
      };
      renderCountdown();
      const timer = setInterval(() => {
        if (renderCountdown() === 0) {
          clearInterval(timer);
          window.__rd.emit({ kind: "timeout" });
        }
      }, 250);
      const finish = (action) => { clearInterval(timer); window.__rd.emit(action); };
      document.getElementById("allow").addEventListener("click", () =>
        finish({ kind: "allow", remember: document.getElementById("remember").checked === true }));
      document.getElementById("deny").addEventListener("click", () => finish({ kind: "deny" }));
    })();
  </script>
</body>
</html>`;
  return dataUrl(locale, html);
}

/** 指示浮层页面：常驻「<设备名> 正在远程控制本机」+「结束」按钮（点击 = 本地结束会话）。 */
export function indicatorWindowDataUrl(locale: Locale, deviceLabel: string): string {
  const text = escapeHtml(remoteDesktopCopy(locale, "remoteDesktop.indicator.text", { device: deviceLabel }));
  const end = escapeHtml(remoteDesktopCopy(locale, "remoteDesktop.indicator.end"));
  const html = `<!doctype html>
<html lang="${locale}">
<head>
  <meta charset="utf-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'">
  <style>${STATE_STYLE}</style>
</head>
<body>
  <div class="card indicator">
    <span class="text" role="status" aria-live="polite">${text}</span>
    <button id="end">${end}</button>
  </div>
  <script>
    ${ACTION_BRIDGE_SCRIPT}
    document.getElementById("end").addEventListener("click", () => window.__rd.emit({ kind: "end" }));
  </script>
</body>
</html>`;
  return dataUrl(locale, html);
}
