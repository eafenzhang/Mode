/* eslint-disable max-lines -- Task 7 重构为单测三段（raw 原语 + runtime 九步 + 双层收尾），
   场景与逐条断言平铺；与同目录 runtime-*.test.mjs 的 max-lines 处置一致。 */
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
//
// 第二段（Task 7）：runtime 级真链路——结构决策为**重构单测双段**（brief 允许「保留 raw
// 步骤或重构，报告说明」）：单 helper + 单 notepad 实例上，Section A 走原 raw-broker 原语
// 往返（逐条保留），Section B 走 createComputerUseRuntime().execute 的工具面九步
//（list_apps → 纯截图观察 → 带树观察 → left_click+type → 读回 → request_access →
// stop/controller_busy），Section C 一次杀 helper 后**双层断言**（raw 层 callBrokerMethod
// 拒绝 + runtime 层热路径 stale_socket）。为何不拆两个 test：第二个 test 需再起一只能否
// 取得前台完全依赖 CreateProcess 一次性前台激活，实测该激活在杀掉第一只记事本后不再发生
//（focused:false 持续 12s+、点击循环全程 foreground_required，plain node 与 node --test
// 均可复现；第一只则恒成功）——单实例方案只走已被 Section A 证明的首次激活路径。
// 同一 env 门（win32 + CUA_INTEGRATION=1 + dist 存在）；teardown 复用同款双收尸。
// 观察类步骤沿用本文件的「有界轮询等就绪、事实逐条断言」口径：轮询只容忍瞬时失败
//（记录末次观测），成功后形状/契约断言一条不放松。
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
import {
  OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY,
  OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES,
  isOfficialCuaImageRefText,
} from "../frame-contract.js";
import { createComputerUseRuntime } from "../index.js";

const entry = fileURLToPath(new URL("../helper/entry.mjs", import.meta.url));
const addonPath = fileURLToPath(new URL("../dist-cua-helper/cua_ax.node", import.meta.url));
const CONTROL_PROTOCOL = "mode-cua-windows-dev/v1";
const TYPED_TEXT = "cua-integration";
const RUNTIME_TYPED_TEXT = "cua-runtime-e2e";

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
  "端到端往返：Section A raw broker 原语（health/launch/observe/click+type/value）+ Section B runtime execute 九步 + Section C 杀 helper 双层断言",
  { skip, timeout: 300_000 },
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

      // ═══════════ Section B（Task 7）：runtime execute 九步真链路 ═══════════
      // 同一 helper/notepad 实例继续（前台事实由 Section A 的成功注入证明并持续持有——
      // 第二实例的 CreateProcess 一次性前台激活不可依赖，见文件头结构决策注记）。
      const runtime = createComputerUseRuntime({ brokerSocketPath: socketPath });
      const context = {
        sessionId: "task7-runtime-e2e",
        runtimeScope: "main",
        workspaceKey: "ws/task7",
        workspacePath: "C:/ws/task7",
      };
      const exec = (toolName, args) =>
        runtime.execute({ toolName, arguments: args ?? {}, context });

      // B1) list_apps：单文本块 = 裸 JSON 数组，launch 后含 notepad 行（按 pid 核名）。
      const list = await exec("list_apps");
      assert.equal(list.isError, false, JSON.stringify(list));
      assert.equal(list.content.length, 1, "list_apps 单文本块");
      assert.equal(list.content[0].type, "text");
      assert.equal("structuredContent" in list, false, "list_apps 无 structuredContent");
      const rows = JSON.parse(list.content[0].text);
      assert.ok(Array.isArray(rows), `应为裸 JSON 数组: ${list.content[0].text.slice(0, 200)}`);
      assert.ok(
        rows.some((row) => row.pid === notepadPid),
        `list_apps 缺 notepad 行(pid=${notepadPid}): ${JSON.stringify(rows).slice(0, 400)}`,
      );

      // B2) 纯截图观察（树隐藏）：帧对 + integrity meta + base64 ≤200KiB + 无树文本块。
      const shot = await untilResult(
        async () => {
          const r = await exec("get_app_state", {
            app_ref: { pid: notepadPid },
            include_screenshot: true,
            tree_shown_to_model: false,
          });
          return r.isError === true
            ? { ok: false, error: JSON.parse(r.content[0].text) }
            : { ok: true, result: r };
        },
        { timeoutMs: 30_000, label: "纯截图观察成功" },
      ).then((s) => s.result);
      assert.equal(
        shot.content.length,
        2,
        `纯截图观察恰两块 image@0+ref@1（无树文本）: ${shot.content.length}`,
      );
      assert.ok(shot.content.length <= 3, "3 块上限");
      assert.equal(shot.content[0].type, "image");
      assert.equal(typeof shot.content[0].data, "string");
      assert.ok(
        isOfficialCuaImageRefText(shot.content[1].text),
        `ref 文本契约: ${String(shot.content[1].text).slice(0, 160)}`,
      );
      assert.ok(
        shot._meta?.[OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY],
        "integrity meta 必须真值（hasOfficialCuaFrameAuthority 只查存在性）",
      );
      const shotBytes = Buffer.byteLength(shot.content[0].data, "base64");
      assert.ok(
        shotBytes > 0 && shotBytes <= OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES,
        `截图 base64 ≤ 200KiB（实际 ${shotBytes}）`,
      );
      assert.match(shot.structuredContent.state_id, /^[0-9a-f-]{36}$/u);

      // B3) 带树观察：header 正则（UI cuaResultState.ts:28 逐字）+ [idx] 行 + 结构化字段。
      const HEADER_RE = /^app:\s+([A-Za-z0-9.-]+)\s+pid=\d+\s+"([^"\r\n]+)"\s*$/mu;
      const shown = await untilResult(
        async () => {
          const r = await exec("get_app_state", {
            app_ref: { pid: notepadPid },
            tree_shown_to_model: true,
          });
          if (r.isError === true) return { ok: false, error: JSON.parse(r.content[0].text) };
          const elements = r.structuredContent.elements;
          const found = elements.find((e) => e.kind === "edit");
          const clickable =
            found &&
            Array.isArray(found.bounds) &&
            found.bounds.length === 4 &&
            found.bounds[2] > 0 &&
            found.bounds[3] > 0;
          return clickable
            ? { ok: true, result: r, edit: found }
            : { ok: false, waiting: `edit 未就绪（elements=${elements.length}）` };
        },
        { timeoutMs: 30_000, label: "带树观察就绪且含可点击 edit" },
      );
      const treeResult = shown.result;
      const targetEdit = shown.edit;
      const treeText = treeResult.content.at(-1).text;
      assert.equal(treeResult.content.at(-1).type, "text", "树文本是末块");
      assert.ok(
        HEADER_RE.test(treeText),
        `header 不匹配: ${JSON.stringify(treeText.split("\n")[0])}`,
      );
      const rowLines = treeText
        .split(/\r?\n/u)
        .filter((line) => /^\s*\[\d+\]\s+(.+)$/u.test(line));
      assert.ok(rowLines.length > 0, `缺 [idx] 元素行: ${treeText.slice(0, 300)}`);
      const sc = treeResult.structuredContent;
      assert.match(sc.state_id, /^[0-9a-f-]{36}$/u, "structuredContent.state_id");
      assert.ok(Array.isArray(sc.elements) && sc.elements.length > 0, "elements 非空");
      assert.equal(typeof sc.window.window_id, "number");
      assert.ok(sc.window.window_id > 0, "window.window_id");
      assert.equal(sc.snapshot_mode, "full", "截图后首观察强制全量");
      t.diagnostic(
        `shown tree: state=${sc.state_id} elements=${sc.elements.length} edit idx=${targetEdit.index}`,
      );

      // B4) left_click（B3 的 element 目标，位移门放行）+ type → 收据 action_sent:true。
      const clicked = await executeWhenReady(exec, "left_click", {
        target: { type: "element", index: targetEdit.index },
        app_ref: { pid: notepadPid },
      });
      if (clicked.isError === true) {
        // 失败现场：notepad 窗口的 focused 标志（apps.rs: focused = GetForegroundWindow）
        // 与存活的 notepad 进程——前台类失败据此定位「谁占着前台/是否多实例」。
        const winRows = await callBrokerMethod({
          socketPath,
          method: "list_windows",
          params: { pid: notepadPid },
          timeoutMs: 5_000,
        }).catch((e) => ({ error: String(e.message) }));
        const procs = spawnSync("tasklist", [
          "/FI",
          "IMAGENAME eq notepad.exe",
          "/FO",
          "CSV",
          "/NH",
        ], { encoding: "utf8" });
        t.diagnostic(
          `foreground evidence: rows=${JSON.stringify(winRows)} notepad=${JSON.stringify((procs.stdout ?? "").trim())}`,
        );
      }
      assert.equal(
        clicked.isError,
        false,
        `left_click: ${JSON.stringify(clicked).slice(0, 300)}`,
      );
      assert.equal(clicked.structuredContent.action_sent, true, "left_click 收据 action_sent");
      assert.equal(
        clicked.structuredContent.dispatch_status,
        "delivered",
        "left_click delivered",
      );

      const typed = await executeWhenReady(exec, "type", {
        text: RUNTIME_TYPED_TEXT,
        app_ref: { pid: notepadPid },
      });
      assert.equal(typed.isError, false, `type: ${JSON.stringify(typed).slice(0, 300)}`);
      assert.equal(typed.structuredContent.action_sent, true, "type 收据 action_sent");
      assert.ok(
        ["delivered", "possibly_sent"].includes(typed.structuredContent.dispatch_status),
        typed.structuredContent.dispatch_status,
      );

      // B5) 重观察（带树）→ edit value 含键入串（SendInput 被应用消化有延迟，轮询取证）。
      const readback = await untilResult(
        async () => {
          const r = await exec("get_app_state", { app_ref: { pid: notepadPid } });
          if (r.isError === true) return { ok: false, error: JSON.parse(r.content[0].text) };
          const found = r.structuredContent.elements.find((e) => e.kind === "edit");
          return found?.value?.includes(RUNTIME_TYPED_TEXT)
            ? { ok: true, value: found.value }
            : { ok: false, waiting: `edit value=${JSON.stringify(found?.value ?? null)}` };
        },
        { timeoutMs: 20_000, label: `edit value 含 "${RUNTIME_TYPED_TEXT}"` },
      );
      assert.ok(readback.value.includes(RUNTIME_TYPED_TEXT));
      t.diagnostic(`runtime readback: ${JSON.stringify(readback.value)}`);

      // B6) request_access：扁平 AccessStatus 文本 + windows structuredContent，无 darwin meta。
      const access = await exec("request_access");
      assert.equal(access.isError, false, JSON.stringify(access));
      assert.deepEqual(JSON.parse(access.content[0].text), {
        ready: true,
        accessibility: "granted",
        screenRecording: "granted",
      });
      assert.equal(access.structuredContent.platform, "windows");
      assert.equal(access.structuredContent.backend, "uia");
      assert.equal("_meta" in access, false, "Windows 不设 darwin-only meta");

      // B7) stop_computer_control → 变更类动作 controller_busy（stop 闸门先于分发）。
      const stop = await exec("stop_computer_control");
      assert.equal(stop.isError, false, JSON.stringify(stop));
      assert.equal(stop.structuredContent.action_sent, true, "stop 本地收据");
      const busy = await exec("left_click", {
        target: { type: "element", index: targetEdit.index },
        app_ref: { pid: notepadPid },
      });
      assert.equal(busy.isError, true, "stop 后 left_click 必须被拒");
      const busyPayload = JSON.parse(busy.content[0].text);
      assert.equal(busyPayload.code, "controller_busy");
      assert.ok(
        busyPayload.message.includes("computer control was stopped"),
        busyPayload.message,
      );

      // ═══════════ Section C：杀 helper → 双层断言（原 raw step 8 + runtime 热路径）═══════════
      // 冷热判定（读 index.js Task 2 实现）：本链首次 execute 已成功 health → healthOk=true
      // 粘住，preflight 直接返回 {kind:"ok"}（不回连、不重打 health）；CUA_NOT_READY 信封只在
      // 「从未成功 health」的冷路径（runHealthPreflight 连接类失败）产生。热身后断连 →
      // handler 内 call 抛 stale_socket（broker.js:169-170 把 socket error/close 一律缠成
      // stale_socket）→ errorResultOf 出 isError:true 17 码错误。
      await reap(child);
      await assert.rejects(
        callBrokerMethod({ socketPath, method: "health", timeoutMs: 2_000 }),
        (error) =>
          error instanceof BrokerError &&
          (error.code === "stale_socket" || /closed|ECONN|ENOENT/i.test(error.message)),
        "helper 死亡后 callBrokerMethod 应以 stale_socket/closed 拒绝",
      );
      t.diagnostic("socket dead: callBrokerMethod rejected after kill");
      const dead = await exec("list_apps");
      assert.equal(dead.isError, true, "热身后断连必须是 error 结果（非冷启动信封）");
      const deadPayload = JSON.parse(dead.content[0].text);
      assert.equal(deadPayload.code, "stale_socket", JSON.stringify(deadPayload));
      t.diagnostic("runtime hot path: stale_socket after helper kill");
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

// ────────────────────────────────────────────── Task 7 helper：有界轮询 / 前台重试

// Section B 使用：有界轮询直到 fn 返回 {ok:true, ...}；超时抛错并带最后一次观测（含末次错误载荷，失败
// 现场不丢）——与 until 同哲学，但允许 fn 返回 {ok:false, error} 表达「瞬时失败、继续等」。
async function untilResult(fn, { timeoutMs, stepMs = 300, label }) {
  const deadline = Date.now() + timeoutMs;
  let last;
  for (;;) {
    last = await fn();
    if (last?.ok) return last;
    if (Date.now() >= deadline) {
      throw new Error(
        `超时（${timeoutMs}ms）未满足：${label}；最后一次观测=${JSON.stringify(last)}`,
      );
    }
    await sleep(stepMs);
  }
}

// 变更类动作的前台就绪期重试：只容忍 foreground_required（与 raw 段 performDispatched
// 同口径），有界；其它错误码原样返回让断言带现场失败，不注水。
async function executeWhenReady(exec, toolName, args, { retryMs = 15_000 } = {}) {
  const deadline = Date.now() + retryMs;
  for (;;) {
    const result = await exec(toolName, args);
    if (result.isError !== true) return result;
    let payload;
    try {
      payload = JSON.parse(result.content[0].text);
    } catch {
      return result;
    }
    if (payload.code === "foreground_required" && Date.now() < deadline) {
      await sleep(300);
      continue;
    }
    return result;
  }
}
