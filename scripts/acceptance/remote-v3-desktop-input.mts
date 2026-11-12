/* oxlint-disable eslint(max-lines) -- 验收脚本按真人操作顺序线性展开；拆分会让「输入→宿主效果」的断言上下文割裂。 */
/**
 * 远程控制 v3 · 桌面输入本机模拟验收（人工触发，不进 CI）。
 *
 * 与 `remote-v3-web.mts` 的分工：那套用假 GitHub + 轻量 Host 验目录/隧道/RPC；
 * 本脚本要验**真实桌面输入**，所以需要一个真桌面 Host（默认取已登录的安装版
 * Mode）和一个已登录的浏览器 profile。
 *
 * 做法：
 * 1. 在宿主 Windows 上起一个 WinForms 输入靶窗口（输入框内容同步到窗口标题），
 *    窗口标题即断言依据，不依赖 UIA（Win11 记事本的控件树读不到 ValuePattern）。
 * 2. 浏览器进远程桌面 → 鼠标移动/点击 → 键盘输入/组合键/退格 → 回读宿主标题。
 * 3. 同一 profile 切手机视口，验证无横向滚动与触控点击映射。
 *
 * 前置：
 * 1. `pnpm --filter @mode/web exec vite --port 5199 --strictPort`
 * 2. 本机安装 Chrome；`.plan/web-profile` 已登录目标账号（可用 ACCEPT_PROFILE 覆盖）
 * 3. 目标 Host 在线（可用 ACCEPT_HOST 覆盖）
 *
 * 运行：`npx tsx scripts/acceptance/remote-v3-desktop-input.mts`
 * 截图落在 `.plan/local-accept/`（本地工作区，不入库）。
 */
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { mkdir, writeFile } from "node:fs/promises";

import { chromium, type BrowserContext, type Page } from "playwright-core";

const WEB_URL = process.env.ACCEPT_WEB_URL ?? "http://localhost:5199/v3";
const HOST_NAME = process.env.ACCEPT_HOST ?? "真机验收 Host";
const PROFILE = process.env.ACCEPT_PROFILE ?? ".plan/web-profile";
const SHOT_DIR = process.env.ACCEPT_SHOT_DIR ?? ".plan/local-accept";
const TITLE_PREFIX = "MODE-INPUT-TARGET|";

const results: Array<{ step: string; ok: boolean; detail?: string }> = [];

function record(step: string, ok: boolean, detail?: string): void {
  results.push({ step, ok, ...(detail === undefined ? {} : { detail }) });
  console.log(`[input] ${ok ? "PASS" : "FAIL"} ${step}${detail ? `：${detail}` : ""}`);
}

function powershell(script: string): string {
  return execFileSync(
    "powershell",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script],
    { encoding: "utf8" },
  ).trim();
}

/** 非关键步骤（置前、恢复指针）失败不该中断整轮验收，只记一行说明。 */
function tryPowershell(script: string): void {
  try {
    powershell(script);
  } catch (error) {
    console.log(`[input] INFO 宿主辅助命令失败：${String(error).slice(0, 120)}`);
  }
}

function cursorPosition(): { x: number; y: number } {
  const [x, y] = powershell(
    "Add-Type -AssemblyName System.Windows.Forms; $p=[System.Windows.Forms.Cursor]::Position; \"$($p.X),$($p.Y)\"",
  )
    .split(",")
    .map(Number);
  return { x, y };
}

function targetTitle(): string {
  return powershell(
    "$p=Get-Process | Where-Object { $_.MainWindowTitle -like 'MODE-INPUT-TARGET*' } | Select-Object -First 1; if($p){ $p.MainWindowTitle } else { '<none>' }",
  );
}

/**
 * 输入靶窗口：最大化 TextBox + 标题同步。标题是跨进程可读的稳定契约，
 * 不依赖控件树（Win11 记事本、部分 Electron 应用的 UIA 树拿不到文本）。
 */
const TARGET_WINDOW_PS1 = String.raw`
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$form = New-Object System.Windows.Forms.Form
$form.Text = "MODE-INPUT-TARGET|"
$form.WindowState = "Maximized"
# 置顶：远程点击按屏幕坐标落点，被其他窗口盖住时会点到别的窗口上。
$form.TopMost = $true
$box = New-Object System.Windows.Forms.TextBox
$box.Multiline = $true
$box.Dock = "Fill"
$box.Font = New-Object System.Drawing.Font("Consolas", 20)
$box.Add_TextChanged({ $form.Text = "MODE-INPUT-TARGET|" + $box.Text })
$form.Controls.Add($box)
[void]$form.Show()
$box.Focus()
while ($form.Visible) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 120 }
`;

async function startTargetWindow(): Promise<{ stop(): void }> {
  const scriptPath = ".plan/input-target.ps1";
  await mkdir(".plan", { recursive: true });
  await writeFile(scriptPath, TARGET_WINDOW_PS1, "utf8");
  // 用 Start-Process 起独立进程：直接 spawn 的 PowerShell 拿不到交互桌面，WinForms 窗口不会出现。
  const absolute = resolve(scriptPath);
  execFileSync("powershell", [
    "-NoProfile",
    "-Command",
    `Start-Process powershell -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','${absolute}'`,
  ]);
  // 等窗口出现（标题就是契约）。
  for (let i = 0; i < 30; i += 1) {
    if (targetTitle().startsWith(TITLE_PREFIX)) {
      return {
        stop: () => {
          powershell(
            "Get-Process | Where-Object { $_.MainWindowTitle -like 'MODE-INPUT-TARGET*' } | Stop-Process -Force",
          );
        },
      };
    }
    await new Promise((resolve) => setTimeout(resolve, 300));
  }
  throw new Error("输入靶窗口未启动");
}

/**
 * 画布用 `object-fit: contain`：元素比视频高/宽时会留黑边，比例必须按**内容区**算，
 * 否则触控/鼠标落点会跟着黑边偏移（本脚本曾因此在手机视口上误判映射错误）。
 */
async function videoContentRect(
  page: Page,
  box: { x: number; y: number; width: number; height: number },
): Promise<{ x: number; y: number; width: number; height: number }> {
  const intrinsic = await page.evaluate(() => {
    const canvas = document.querySelector("canvas");
    return canvas ? { width: canvas.width, height: canvas.height } : null;
  });
  if (!intrinsic || intrinsic.width === 0 || intrinsic.height === 0) return box;
  const scale = Math.min(box.width / intrinsic.width, box.height / intrinsic.height);
  const width = intrinsic.width * scale;
  const height = intrinsic.height * scale;
  return {
    x: box.x + (box.width - width) / 2,
    y: box.y + (box.height - height) / 2,
    width,
    height,
  };
}

/** 会话面挂载后会插入操作栏/状态条，画布尺寸会跳一次；取点前必须等它稳定。 */
async function waitForStableCanvas(page: Page): Promise<{ x: number; y: number; width: number; height: number }> {
  let last = "";
  let stable = 0;
  for (let i = 0; i < 40; i += 1) {
    const box = await page.locator("canvas").first().boundingBox();
    if (box) {
      const signature = [box.x, box.y, box.width, box.height].map((v) => Math.round(v)).join(",");
      stable = signature === last ? stable + 1 : 0;
      last = signature;
      if (stable >= 2) return box;
    }
    await page.waitForTimeout(150);
  }
  const box = await page.locator("canvas").first().boundingBox();
  if (!box) throw new Error("未找到画面 canvas");
  return box;
}

async function openDesktop(page: Page): Promise<{ box: { x: number; y: number; width: number; height: number } }> {
  const row = page.locator("li", { hasText: HOST_NAME }).first();
  await row.waitFor({ timeout: 30_000 });
  await row.getByRole("button", { name: "进入桌面" }).click();
  await page.getByText("画面已连接").waitFor({ timeout: 60_000 });
  await page.waitForTimeout(2_500);
  const box = await page.locator("canvas").first().boundingBox();
  if (!box) throw new Error("未找到画面 canvas");
  return { box };
}

async function launch(viewport: { width: number; height: number }, mobile = false): Promise<BrowserContext> {
  return chromium.launchPersistentContext(PROFILE, {
    channel: "chrome",
    headless: true,
    viewport,
    ...(mobile ? { deviceScaleFactor: 2, hasTouch: true, isMobile: true } : {}),
    args: ["--allow-loopback-in-peer-connection", "--disable-features=WebRtcHideLocalIpsWithMdns"],
  });
}

async function main(): Promise<void> {
  await mkdir(SHOT_DIR, { recursive: true });
  const origin = cursorPosition();
  const target = await startTargetWindow();
  const context = await launch({ width: 1440, height: 900 });
  try {
    const page = context.pages()[0] ?? (await context.newPage());
    const pageErrors: string[] = [];
    page.on("pageerror", (error) => pageErrors.push(error.message));
    await page.goto(WEB_URL, { waitUntil: "domcontentloaded" });
    await openDesktop(page);
    const size = (await page.locator("body").innerText()).match(/(\d{3,4})×(\d{3,4})/);
    const [hostWidth, hostHeight] = size ? [Number(size[1]), Number(size[2])] : [1920, 1080];

    // 鼠标：画面 25%/25% → 宿主约 1/4 坐标（按视频内容区，避开黑边）。
    const desktopVideo = await videoContentRect(page, await waitForStableCanvas(page));
    await page.mouse.move(desktopVideo.x + desktopVideo.width * 0.25, desktopVideo.y + desktopVideo.height * 0.25, { steps: 12 });
    await page.waitForTimeout(700);
    const moved = cursorPosition();
    const expectX = Math.round(hostWidth * 0.25);
    const expectY = Math.round(hostHeight * 0.25);
    const distance = Math.hypot(moved.x - expectX, moved.y - expectY);
    record(
      "鼠标移动映射到宿主坐标",
      distance <= 60,
      `宿主 ${moved.x},${moved.y} ≈ ${expectX},${expectY}（偏差 ${Math.round(distance)}px）`,
    );

    // 键盘：先把靶窗口置前。Windows 上「首次点击只激活窗口」，所以点两次：
    // 第一次把窗口提到前台，第二次才把焦点给文本框。仍失败则重试（最多 3 轮），
    // 避免把宿主窗口层级/激活时序的抖动误判成输入缺陷。
    const desktopVideoForClick = await videoContentRect(page, await waitForStableCanvas(page));
    const clickPoint = {
      x: desktopVideoForClick.x + desktopVideoForClick.width * 0.5,
      y: desktopVideoForClick.y + desktopVideoForClick.height * 0.5,
    };
    const text = `MODE-KB-${Date.now().toString().slice(-6)}`;
    for (let attempt = 1; attempt <= 3; attempt += 1) {
      tryPowershell(
        "$p=Get-Process | Where-Object { $_.MainWindowTitle -like 'MODE-INPUT-TARGET*' } | Select-Object -First 1; if (-not $p) { exit 0 }; Add-Type -MemberDefinition '[DllImport(\"user32.dll\")] public static extern bool SetForegroundWindow(IntPtr h);' -Name F -Namespace FG -PassThru | Out-Null; [FG.F]::SetForegroundWindow($p.MainWindowHandle) | Out-Null",
      );
      await page.waitForTimeout(250);
      await page.mouse.click(clickPoint.x, clickPoint.y);
      await page.waitForTimeout(250);
      await page.mouse.click(clickPoint.x, clickPoint.y);
      await page.waitForTimeout(400);
      await page.keyboard.type(text, { delay: 55 });
      await page.waitForTimeout(900);
      if (targetTitle().includes(text)) break;
      console.log(`[input] INFO 第 ${attempt} 轮键盘输入未落到靶窗口，重试`);
    }
    record("键盘输入写入宿主焦点窗口", targetTitle().includes(text), `宿主标题「${targetTitle().slice(0, 60)}」`);

    await page.keyboard.press("Control+End");
    await page.keyboard.type("-TAIL");
    await page.waitForTimeout(700);
    record(
      "组合键 Ctrl+End 后继续输入落在末尾",
      targetTitle().includes(`${text}-TAIL`),
      `宿主标题「${targetTitle().slice(0, 70)}」`,
    );

    const beforeBackspace = targetTitle();
    await page.keyboard.press("Backspace");
    await page.waitForTimeout(600);
    const afterBackspace = targetTitle();
    record(
      "退格键逐字删除（无重复注入）",
      afterBackspace.length === beforeBackspace.length - 1,
      `「${beforeBackspace.slice(-8)}」→「${afterBackspace.slice(-8)}」`,
    );
    await page.screenshot({ path: `${SHOT_DIR}/10-desktop-input.png` });
    if (pageErrors.length) record("会话面无脚本错误", false, pageErrors.slice(0, 3).join(" | "));
  } finally {
    await context.close();
    target.stop();
    tryPowershell(
      `Add-Type -AssemblyName System.Windows.Forms,System.Drawing; [System.Windows.Forms.Cursor]::Position = [System.Drawing.Point]::new(${origin.x}, ${origin.y})`,
    );
  }

  // 手机视口：同一 profile 切移动端，验布局与触控。
  const mobileContext = await launch({ width: 390, height: 844 }, true);
  try {
    const page = mobileContext.pages()[0] ?? (await mobileContext.newPage());
    await page.goto(WEB_URL, { waitUntil: "domcontentloaded" });
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - window.innerWidth,
    );
    record("手机视口无横向滚动", overflow <= 1, `溢出 ${overflow}px`);
    const row = page.locator("li", { hasText: HOST_NAME }).first();
    // 移动 UA 下同一 profile 需要先恢复账号与目录；等首屏稳定再判定，避免把加载态当未登录。
    await row.waitFor({ timeout: 25_000 }).catch(() => undefined);
    if (await row.count()) {
      await openDesktop(page);
      await waitForStableCanvas(page);
      const size = (await page.locator("body").innerText()).match(/(\d{3,4})×(\d{3,4})/);
      const [hostWidth, hostHeight] = size ? [Number(size[1]), Number(size[2])] : [1920, 1080];
      const video = await videoContentRect(page, await waitForStableCanvas(page));
      // 先把指针挪到左上，避免「本来就在落点」让断言假通过。
      await page.mouse.move(video.x + video.width * 0.1, video.y + video.height * 0.1, { steps: 8 });
      await page.waitForTimeout(600);
      const before = cursorPosition();
      await page.touchscreen.tap(video.x + video.width * 0.7, video.y + video.height * 0.6);
      await page.waitForTimeout(900);
      const after = cursorPosition();
      const expect = { x: Math.round(hostWidth * 0.7), y: Math.round(hostHeight * 0.6) };
      const drift = Math.hypot(after.x - expect.x, after.y - expect.y);
      record(
        "触控点击映射到宿主坐标",
        drift <= 60,
        `宿主 ${before.x},${before.y} → ${after.x},${after.y} ≈ ${expect.x},${expect.y}（偏差 ${Math.round(drift)}px）`,
      );
      await page.screenshot({ path: `${SHOT_DIR}/11-mobile-desktop.png` });
    } else {
      record("手机视口进入远程桌面", false, "profile 在移动 UA 下没有登录态");
    }
  } finally {
    await mobileContext.close();
  }

  const failed = results.filter((entry) => !entry.ok).length;
  console.log(
    `[input] summary pass=${results.length - failed} fail=${failed} results=${JSON.stringify(results)}`,
  );
  if (failed > 0) process.exitCode = 1;
}

main().catch((error) => {
  console.error("[input] 失败", error);
  process.exitCode = 1;
});
