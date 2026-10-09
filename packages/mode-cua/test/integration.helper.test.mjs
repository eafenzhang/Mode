// Task 10：端到端集成（CUA_INTEGRATION=1 门）——真 helper（源码 helper/entry.mjs）+
// 真 addon（dist-cua-helper/cua_ax.node）+ 真记事本的完整往返验收。
//
// 往返链：spawn → IPC 握手 → health 合成校验 → screen_probe → launch_app(notepad)
// → list_windows(pid) 等窗口 → observe 找 kind:"edit" → perform click(edit 屏幕绝对坐标)
// + type_text → re-observe 断言 edit value 含键入串 → 杀 helper → callBrokerMethod 抛
// stale_socket → finally 双收尸（helper + 按 pid 核名收割 notepad）。
//
// 与 brief 草图的偏差（详见 task-10-report.md）：
// 1) spawn 源码 helper/entry.mjs 而非打包 entry.cjs：Task 9 已冒烟验证打包产物，本测试验真 addon 端到端；
// 2) 三重跳过门：非 win32 / 未设 CUA_INTEGRATION=1 / dist-cua-helper 产物缺失（services 契约
//    测试同款文案），保证未构建贡献者的 `pnpm --filter @mode/cua test` 只跳过不失败；
// 3) pid=0 → 如实 skip 不重试（Task 6：notepad 走 SearchPathW+CreateProcessW 应回填 >0；
//    重试 relaunch 可能泄漏首个实例——收割无 pid 比跳过更危险）；
// 4) event 路径有 controller 前台闸门（foreground_required 零下发）：记事本 launch 后通常自动
//    取得前台，前台就绪期用有界重试——只容忍 foreground_required，其它错误码立即失败、不注水；
// 5) 附加一行 screen_probe 往返（backend → addon → {locked}，任务方允许的廉价加测）。
import test from "node:test";
import assert from "node:assert/strict";
import { fork, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

import {
  BrokerError,
  callBrokerMethod,
  mintBrokerSocketPath,
  probeHelperHealth,
} from "../broker.js";

const entry = fileURLToPath(new URL("../helper/entry.mjs", import.meta.url));
const addonPath = fileURLToPath(new URL("../dist-cua-helper/cua_ax.node", import.meta.url));
const CONTROL_PROTOCOL = "mode-cua-windows-dev/v1";
const TYPED_TEXT = "cua-integration";

// 跳过理由按优先级取第一条（false = 不跳过）；普通 test 命令在无 env 或未构建时安静跳过。
const skip =
  process.platform !== "win32"
    ? "端到端集成仅在 Windows 运行（真 addon 依赖 UIA/SendInput）"
    : process.env.CUA_INTEGRATION !== "1"
      ? "CUA_INTEGRATION=1 时运行端到端集成（真 helper + 真记事本往返）"
      : !existsSync(addonPath)
        ? "dist-cua-helper 缺失：先运行 pnpm build:cua-helper 生成 CUA helper 产物"
        : false;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// 轮询直到 fn 返回真值；超时抛出并带上最后一次观测值（失败证据不丢现场）。
async function until(fn, { timeoutMs, stepMs = 200, label }) {
  const deadline = Date.now() + timeoutMs;
  let last;
  for (;;) {
    last = await fn();
    if (last) return last;
    if (Date.now() >= deadline) {
      throw new Error(
        `超时（${timeoutMs}ms）未满足：${label}；最后一次观测=${JSON.stringify(last)}`,
      );
    }
    await sleep(stepMs);
  }
}

// teardown：kill + 等 exit；断言失败路径同样收尸，绝不泄漏 helper 子进程
//（与 helper-entry.test.mjs 的 reap 同款：兜底再杀一次放行，测试不因收尸挂死）。
function reap(child) {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise((resolve) => {
    const hardStop = setTimeout(() => {
      child.kill();
      resolve();
    }, 5_000);
    child.once("exit", () => {
      clearTimeout(hardStop);
      resolve();
    });
    if (!child.kill()) {
      clearTimeout(hardStop);
      resolve();
    }
  });
}

// 按 pid 核名收割 notepad（crates/mode-cua-ax/tests/launch_screen.rs reap_notepad 的 JS 对齐：
// 双保险——进程名确认含 notepad、或名字读不出但 pid 来自本次 launch 返回值，才 taskkill /F；
// pid 已退出视为无需收割；名字读得出但不含 notepad 则拒绝误杀）。
function reapNotepad(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return;
  const query = spawnSync("tasklist", ["/FI", `PID eq ${pid}`, "/FO", "CSV", "/NH"], {
    encoding: "utf8",
  });
  const out = (query.stdout ?? "").trim();
  if (!out || out.startsWith("INFO:")) return; // 进程已退出
  const name = out.split(",")[0].replaceAll('"', "").toLowerCase();
  if (!name.includes("notepad")) {
    console.error(`[integration] pid ${pid} 实为 ${name}（非 notepad），拒绝误杀`);
    return;
  }
  spawnSync("taskkill", ["/PID", String(pid), "/F"], { stdio: "ignore" });
}

function awaitReady(child, timeoutMs = 10_000) {
  return new Promise((resolve, reject) => {
    const messages = [];
    const timer = setTimeout(() => reject(new Error(`no ready in ${timeoutMs}ms`)), timeoutMs);
    child.on("message", (m) => {
      messages.push(m);
      if (m?.type === "ready") {
        clearTimeout(timer);
        resolve({ ready: m, messages });
      }
      if (m?.type === "error") {
        clearTimeout(timer);
        reject(new Error(`helper error handshake: ${JSON.stringify(m)}`));
      }
    });
  });
}

// perform 往返：成功必须 dispatched==="dispatched"；仅前台就绪期容忍 foreground_required
//（controller 前台闸门的预期时序），有界重试，其它错误码原样上抛（不注水）。
async function performDispatched(socketPath, params, { retryMs = 15_000 } = {}) {
  const deadline = Date.now() + retryMs;
  for (;;) {
    try {
      const r = await callBrokerMethod({
        socketPath,
        method: "perform",
        params,
        timeoutMs: 15_000,
      });
      assert.equal(
        r.dispatched,
        "dispatched",
        `perform ${params.kind} 未 dispatched: ${JSON.stringify(r)}`,
      );
      return r;
    } catch (error) {
      if (
        error instanceof BrokerError &&
        error.code === "foreground_required" &&
        Date.now() < deadline
      ) {
        await sleep(300);
        continue;
      }
      throw error;
    }
  }
}

test(
  "helper 端到端往返：spawn → handshake → health → launch notepad → observe/edit → click+type → value → kill",
  { skip, timeout: 180_000 },
  async (t) => {
    const socketPath = mintBrokerSocketPath();
    const child = fork(entry, ["--socket", socketPath, "--parent-pid", String(process.pid)], {
      stdio: ["ignore", "pipe", "pipe", "ipc"],
      env: { ...process.env, MODE_CUA_HELPER_ADDON: addonPath },
    });
    // helper 输出只作失败诊断（napi-sys stderr 噪声已知），不污染测试输出。
    let helperOutput = "";
    child.stdout?.on("data", (chunk) => {
      helperOutput += chunk;
    });
    child.stderr?.on("data", (chunk) => {
      helperOutput += chunk;
    });

    let notepadPid = 0;
    try {
      // 1) spawn + IPC 握手（transport_ready/ready 字段契约由 Task 7 单测钉住，这里取 ready 即可）
      const { ready } = await awaitReady(child);
      assert.equal(ready.protocol, CONTROL_PROTOCOL);
      assert.equal(ready.socketPath, socketPath);
      assert.equal(ready.pid, child.pid);
      t.diagnostic(`helper ready: pid=${child.pid} socket=${socketPath}`);

      // 2) health 合成（pid 必须 = helper 子进程自身，绝不来自 addon）
      const health = await probeHelperHealth(socketPath, { timeoutMs: 15_000 });
      assert.equal(health.pid, child.pid, "health.pid 必须等于 helper 子进程 pid");

      // 2b) screen_probe 一行往返（backend → addon → {locked}）
      const probe = await callBrokerMethod({
        socketPath,
        method: "screen_probe",
        timeoutMs: 10_000,
      });
      assert.equal(
        typeof probe.locked,
        "boolean",
        `screen_probe.locked 应为 boolean: ${JSON.stringify(probe)}`,
      );

      // 3) launch_app notepad → pid>0（Task 6：SearchPathW+CreateProcessW 路径应回填真实 pid）
      const app = await callBrokerMethod({
        socketPath,
        method: "launch_app",
        params: { name: "notepad" },
        timeoutMs: 60_000,
      });
      assert.equal(typeof app.pid, "number", `launch_app 返回形状异常: ${JSON.stringify(app)}`);
      if (!(app.pid > 0)) {
        // Task 6 已知收养降级（pid=0）：无 pid 无法核名收割，如实 skip，不盲杀不注水。
        t.skip(`launch_app 未回填 pid（${app.pid}），无法安全收割 notepad，跳过往返断言`);
        return;
      }
      notepadPid = app.pid;
      t.diagnostic(`launch_app notepad: pid=${notepadPid} name=${JSON.stringify(app.name)}`);

      // 4) list_windows(pid) 等记事本主窗口出现（窗口创建晚于进程启动，轮询）
      const win = await until(
        async () => {
          const rows = await callBrokerMethod({
            socketPath,
            method: "list_windows",
            params: { pid: notepadPid },
            timeoutMs: 5_000,
          });
          return rows.find((r) => r.windowId > 0) ?? null;
        },
        { timeoutMs: 20_000, label: `pid=${notepadPid} 出现顶层窗口` },
      );
      // 标题本地化不锁定 "Untitled" 字样（系统等价）：主窗口必须有非空标题。
      assert.equal(typeof win.title, "string", `窗口标题应为 string: ${JSON.stringify(win)}`);
      assert.ok(win.title.length > 0, "记事本主窗口标题为空");
      t.diagnostic(`window: id=${win.windowId} title=${JSON.stringify(win.title)}`);

      // 5) observe → 断言含 kind:"edit"（UIA 树就绪同样需要轮询）
      const { edit } = await until(
        async () => {
          const r = await callBrokerMethod({
            socketPath,
            method: "observe",
            params: { windowId: win.windowId },
            timeoutMs: 10_000,
          });
          const found = r.elements.find((e) => e.kind === "edit");
          return found ? { edit: found, total: r.elements.length } : null;
        },
        { timeoutMs: 20_000, label: "observe 出现 kind=edit 元素" },
      );
      // bounds = [x, y, w, h] 屏幕绝对坐标（UIA CurrentBoundingRectangle）；退化框无法点击。
      assert.equal(
        edit.bounds.length,
        4,
        `edit.bounds 应为 4 元整数: ${JSON.stringify(edit.bounds)}`,
      );
      assert.ok(
        edit.bounds[2] > 0 && edit.bounds[3] > 0,
        `edit.bounds 退化: ${JSON.stringify(edit.bounds)}`,
      );
      const cx = edit.bounds[0] + Math.floor(edit.bounds[2] / 2);
      const cy = edit.bounds[1] + Math.floor(edit.bounds[3] / 2);
      t.diagnostic(
        `edit: index=${edit.index} bounds=${JSON.stringify(edit.bounds)} center=(${cx},${cy})`,
      );

      // 6) perform click(edit 绝对坐标) + type_text → dispatched（前台就绪期有界重试）
      await performDispatched(socketPath, {
        kind: "click",
        windowId: win.windowId,
        payload: { x: cx, y: cy },
      });
      await performDispatched(socketPath, {
        kind: "type_text",
        windowId: win.windowId,
        payload: { text: TYPED_TEXT },
      });

      // 7) re-observe → edit value 含键入串（SendInput 被应用消化有延迟，轮询取证）
      const value = await until(
        async () => {
          const r = await callBrokerMethod({
            socketPath,
            method: "observe",
            params: { windowId: win.windowId },
            timeoutMs: 10_000,
          });
          const found = r.elements.find((e) => e.kind === "edit");
          return found?.value?.includes(TYPED_TEXT) ? found.value : null;
        },
        { timeoutMs: 10_000, label: `edit value 含 "${TYPED_TEXT}"` },
      );
      assert.ok(value.includes(TYPED_TEXT));
      t.diagnostic(`value roundtrip: ${JSON.stringify(value)}`);

      // 8) 杀 helper 子进程 → socket 已死，callBrokerMethod 必须以 stale_socket 拒绝
      await reap(child);
      await assert.rejects(
        callBrokerMethod({ socketPath, method: "health", timeoutMs: 2_000 }),
        (error) =>
          error instanceof BrokerError &&
          (error.code === "stale_socket" || /closed|ECONN|ENOENT|pipe/i.test(error.message)),
        "helper 死亡后 callBrokerMethod 应以 stale_socket/closed 拒绝",
      );
      t.diagnostic("socket dead: callBrokerMethod rejected after kill");
    } catch (error) {
      // 断言失败的现场证据：附上 helper 进程输出尾部（napi-sys 噪声为已知噪声）。
      if (helperOutput) {
        error.message += `\n--- helper output (tail) ---\n${helperOutput.slice(-4000)}`;
      }
      throw error;
    } finally {
      // 双收尸覆盖所有路径（含断言失败/超时）：helper 优先（若尚未杀），再收割 notepad。
      await reap(child);
      reapNotepad(notepadPid);
    }
  },
);
