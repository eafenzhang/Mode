/* Mode 官网交互：主题 / 语言切换、终端动画、进场动画、版本号拉取。 */
(() => {
  "use strict";

  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
  const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;

  /* ---------- 主题 ---------- */
  const themeKey = "mode-site-theme";
  const preferred = localStorage.getItem(themeKey)
    || (matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark");
  document.documentElement.dataset.theme = preferred;

  $("#themeToggle").addEventListener("click", () => {
    const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    localStorage.setItem(themeKey, next);
  });

  /* ---------- 语言 ----------
     中文文案以 HTML 为准（首扫描进 zhMap），英文来自下方字典。 */
  const zhMap = new Map();
  $$("[data-i18n]").forEach((el) => zhMap.set(el, el.textContent));
  $$("[data-i18n-html]").forEach((el) => zhMap.set(el, el.innerHTML));

  const EN = {
    "skip": "Skip to content",
    "nav.features": "Features",
    "nav.surfaces": "Surfaces",
    "nav.download": "Download",
    "nav.star": "Star",
    "hero.eyebrow1": "Open source",
    "hero.eyebrow2": "MIT License",
    "hero.eyebrow3": "Rust core",
    "hero.title": "Plans. Edits. Runs. <em>Verifies.</em>",
    "hero.sub": `Mode is a coding agent that lives on your desktop: every change shown as a diff,
      every command run in context, every result re-checked by tests.
      Desktop, browser and terminal — one agent, one set of sessions.`,
    "hero.download": "Download Mode",
    "hero.source": "View source",
    "hero.meta": "Windows · macOS · Linux　Unsigned builds, sha256 checksums and silent updates",
    "features.title": "A workspace built for long sessions",
    "features.sub": "Calm, dense and operational — every capability is built around one loop: approve before it acts, verify after it's done.",
    "f1.h": "Self-verifying",
    "f1.p": "It re-runs your tests after editing and brings results back into the session. Every change is presented as a diff — files are never silently rewritten.",
    "f2.h": "Asks before it acts",
    "f2.p": "Every edit, command and tool call can require approval — just this once, always in this project, or fully unlocked.",
    "f3.h": "One agent, three surfaces",
    "f3.p": "The Tauri desktop app, the browser workspace and the terminal TUI share one runtime and the same sessions.",
    "f4.h": "IM bots",
    "f4.p": "Drive your workspace from WeChat / WeCom / Feishu / DingTalk, with per-person DMs and per-group threads bound to separate sessions.",
    "f5.h": "LAN remote",
    "f5.p": "Modes on the same network discover each other via UDP broadcast; a one-time 6-digit pairing code grants access.",
    "f6.h": "Remote agent",
    "f6.p": "A single-file mode-agent on Windows accepts commands over MCP Streamable HTTP: shell, files, processes, screenshots, keyboard and mouse.",
    "f7.h": "Plugins, skills & MCP",
    "f7.p": "Built-in plugin marketplace with an offline public catalog; MCP servers configured per user or per workspace.",
    "f8.h": "Bring your own model",
    "f8.p": "Presets for DeepSeek, OpenAI, Anthropic, Kimi, GLM, xAI and more — plus fully custom endpoints.",
    "models.label": "Model presets out of the box",
    "srf.title": "Desktop, browser, terminal — the same sessions",
    "srf.sub": "Workspace state, approvals and run results stay consistent across surfaces; remote workspaces support SSH, WSL and Docker.",
    "s1.h": "Desktop",
    "s1.p": "A Rust + Tauri v2 shell: windows, tray, deep links, updates and Host sidecar supervision all run locally.",
    "s2.h": "Browser",
    "s2.p": "The browser workspace shares the Host attachment with phone remote control, so long tasks stay in reach.",
    "s3.h": "Terminal TUI",
    "s3.p": "mode-cli brings the same runtime to your shell — the native entry point for scripting and keyboard-driven work.",
    "dl.title": "Download Mode",
    "dl.sub": "Installers ship on GitHub Releases. Builds are not code-signed, so your system will warn once on first launch — that's expected; verify against sha256.txt on the release page.",
    "dl.win": "On SmartScreen: More info → Run anyway",
    "dl.mac": "Remove the quarantine attribute first (see release notes)",
    "dl.linux": "chmod +x, then run directly",
    "dl.note": "Installed desktop apps update themselves from GitHub Releases. File names and checksums always follow the Releases page.",
    "foot.tag": "An AI coding workspace for desktop, browser and terminal — its core rewritten in Rust.",
    "foot.project": "Project",
    "foot.releases": "Releases",
    "foot.legal": "License",
    "foot.copy": "© 2026 Mode contributors · Open source under the MIT license",
  };

  let lang = localStorage.getItem("mode-site-lang") || "zh";

  function applyLang(next) {
    lang = next;
    localStorage.setItem("mode-site-lang", next);
    document.documentElement.lang = next === "zh" ? "zh-CN" : "en";
    $$("[data-i18n]").forEach((el) => {
      const key = el.dataset.i18n;
      el.textContent = next === "zh" ? zhMap.get(el) : (EN[key] ?? zhMap.get(el));
    });
    $$("[data-i18n-html]").forEach((el) => {
      const key = el.dataset.i18nHtml;
      el.innerHTML = next === "zh" ? zhMap.get(el) : (EN[key] ?? zhMap.get(el));
    });
    $("#langToggle").textContent = next === "zh" ? "EN" : "中";
  }

  $("#langToggle").addEventListener("click", () => applyLang(lang === "zh" ? "en" : "zh"));
  applyLang(lang);

  /* ---------- 终端动画（签名元素） ---------- */
  const termBody = $("#termBody");

  const TERM_ZH = [
    [{ t: "$ ", c: "t-prompt" }, { t: 'mode "修复 useSession 的定时器泄漏"', c: "t-cmd", type: true }],
    [{ t: "◆ 计划 · 3 步", c: "t-plan" }],
    [{ t: "  1  定位 interval 创建处", c: "t-step" }],
    [{ t: "  2  组件卸载时清除定时器", c: "t-step" }],
    [{ t: "  3  重跑相关测试", c: "t-step" }],
    [{ t: "● Edit  ", c: "t-plan" }, { t: "packages/ui/src/hooks/useSession.ts  ", c: "t-step" }, { t: "+3 −1", c: "t-diff-b" }],
    [{ t: "● Bash  ", c: "t-plan" }, { t: "pnpm vitest run useSession", c: "t-step" }],
    [{ t: "✓ 12 passed · 1.8s", c: "t-ok" }],
    [{ t: "⏸ 等待审批  ", c: "t-approve" }, { t: "[仅本次允许] [本项目内一直允许]", c: "t-dim" }],
    [{ t: "  已允许 · 仅本次 —— 会话继续", c: "t-step" }],
  ];

  const TERM_EN = [
    [{ t: "$ ", c: "t-prompt" }, { t: 'mode "fix timer leak in useSession"', c: "t-cmd", type: true }],
    [{ t: "◆ plan · 3 steps", c: "t-plan" }],
    [{ t: "  1  locate the interval creation", c: "t-step" }],
    [{ t: "  2  clear it on unmount", c: "t-step" }],
    [{ t: "  3  re-run related tests", c: "t-step" }],
    [{ t: "● Edit  ", c: "t-plan" }, { t: "packages/ui/src/hooks/useSession.ts  ", c: "t-step" }, { t: "+3 −1", c: "t-diff-b" }],
    [{ t: "● Bash  ", c: "t-plan" }, { t: "pnpm vitest run useSession", c: "t-step" }],
    [{ t: "✓ 12 passed · 1.8s", c: "t-ok" }],
    [{ t: "⏸ awaiting approval  ", c: "t-approve" }, { t: "[just once] [always here]", c: "t-dim" }],
    [{ t: "  approved · just once — session continues", c: "t-step" }],
  ];

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  function renderStatic(lines) {
    termBody.replaceChildren();
    for (const segs of lines) {
      const ln = document.createElement("span");
      ln.className = "ln";
      for (const s of segs) {
        const span = document.createElement("span");
        span.className = s.c;
        span.textContent = s.t;
        ln.appendChild(span);
      }
      termBody.appendChild(ln);
    }
    const cur = document.createElement("span");
    cur.className = "ln";
    cur.innerHTML = '<span class="t-cursor"> </span>';
    termBody.appendChild(cur);
  }

  async function typeLoop() {
    for (;;) {
      const script = lang === "zh" ? TERM_ZH : TERM_EN;
      termBody.replaceChildren();
      for (const segs of script) {
        const ln = document.createElement("span");
        ln.className = "ln";
        termBody.appendChild(ln);
        for (const s of segs) {
          const span = document.createElement("span");
          span.className = s.c;
          ln.appendChild(span);
          if (s.type && !reducedMotion) {
            for (const ch of s.t) {
              span.textContent += ch;
              await sleep(26);
            }
          } else {
            span.textContent = s.t;
            await sleep(reducedMotion ? 0 : 130);
          }
        }
      }
      const cur = document.createElement("span");
      cur.className = "t-cursor";
      cur.textContent = " ";
      termBody.appendChild(cur);
      await sleep(3400);
    }
  }

  if (reducedMotion) {
    renderStatic(TERM_ZH);
  } else {
    typeLoop();
  }

  /* ---------- 进场动画 ---------- */
  const targets = $$(".section-head, .feature, .surface, .dl-card, .model-strip");
  if (reducedMotion || !("IntersectionObserver" in window)) {
    targets.forEach((el) => el.classList.add("in"));
  } else {
    targets.forEach((el) => el.classList.add("reveal"));
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            e.target.classList.add("in");
            io.unobserve(e.target);
          }
        }
      },
      { threshold: 0.12 }
    );
    targets.forEach((el) => io.observe(el));
  }

  /* ---------- 最新版本号 ----------
     版本号由 NAS 端静态服务定时从 GitHub API 刷新到站点目录（见 website/deploy/nas-static.js），
     页面只做同源读取，不依赖浏览器直连 GitHub（国内网络不可靠）。 */
  const chip = $("#versionChip");
  fetch("/version.json", { cache: "no-store" })
    .then((r) => (r.ok ? r.json() : null))
    .then((d) => {
      const tag = d && typeof d.tag === "string" ? d.tag : null;
      if (!tag) return;
      chip.textContent = tag;
      chip.hidden = false;
      const ver = tag.replace(/^v/, "");
      $$(".dl-card code").forEach((el) => {
        el.textContent = el.textContent.replace("<版本>", ver);
      });
    })
    .catch(() => {});
})();
