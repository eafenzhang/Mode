# Computer Use Host Runtime（Plan B）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让桌面端输入框的「电脑控制」真正点亮——`@mode/cua` 运行时真现实现（14 工具、状态/帧/收据、CUA_NOT_READY）+ resolver 凭据链，端到端从 `mcp__node_repl__js` 打到真 Helper/Addon。

**Architecture:** 三层已在 Plan A 落地（broker 协议 / Helper+backend / Rust addon）。Plan B 只做 host 侧两件事：(1) `packages/mode-cua/index.js` 的 `createComputerUseRuntime`——会话（`session_id|workspace_key` 键）、观察渲染与 diff、位移台账、帧对发射、收据与 17 码错误装配；(2) `broker-server.js` 的 resolver 关键面真现实现——插件启用才拉起 host、凭据注入 MCP env、mac 分支保持 fail-closed stub。

**Tech Stack:** 纯 ESM Node（零依赖，`node:` 内置）、既有 `broker.js` 客户端；少量 Rust（截图 ≤200KiB 阶梯，Task 6）。

**Spec:** `docs/specs/computer-use-windows-runtime.md`（帧契约/工具面/错误模型的规范源）；Plan A 计划 `docs/superpowers/plans/2026-10-09-computer-use-capability-layer.md`（能力层已完成，其 Global Constraints 继续生效）。

**规范材料（clean-room 允许面，禁止读官方安装件实现）：**
- 插件 SDK（固定、契约级）：`C:\Users\Administrator\.zcode\cli\plugins\cache\zcode-plugins-official\computer-use\0.6.3\scripts\computer-use-client.mjs`
- 插件文档（固定）：同目录 `docs\computer-use.md`、`skills\computer-use\SKILL.md`
- 本仓消费方：`apps/mode-cli/packages/core`（result-display/image-normalization/result-content-projection/call-runner）、`apps/mode-cli/packages/node-repl-host/src/{result,cua-bridge,server}.ts`、`packages/ui/src/ToolCallBlocks/renderers/cua*`

## Global Constraints

- 纯净室：只用上面列出的规范材料 + 本仓消费方 + Windows 公开 API；**不读** `C:\Users\Administrator\AppData\Local\Programs\ZCode` 下任何文件。
- 错误码 17 键集合（spec §runtime 面；与 `crates/mode-cua-ax/src/error.rs` `ALLOWED_CODES`、`helper/errors.mjs` 同源）；SDK `ERROR_CODE_BY_BROKER` 固定映射。
- `packages/mode-cua` 零运行时依赖（`node:` 内置 + 包内互引）；**不得**反向依赖 core/services/ui；`mode-cua` 不改 `broker.d.ts` 既有导出面（只允许追加：`protocolVersion?: string` 到 `HelperHealth`、帧构建器两个新导出+声明）。
- win32 一期；mac 专属 stub（`createProductCuaHelperHost`、TCC 请求族等）保持 fail-closed 不动。
- 提交只 `git add` 本任务文件（工作树常有并行会话改动：NOTICE/README/patches/scripts-license 等，绝不用 `git add -A`）；绝不 push。
- 测试命令：`pnpm --filter @mode/cua test`（node --test）、`pnpm --filter @mode/services test`、`CUA_INTEGRATION=1 pnpm --filter @mode/cua run test:integration`；全仓口径 `pnpm typecheck` / `pnpm lint` / `pnpm architecture:check --changed`，报告真实结果。
- 已裁决语义（来自 Plan A 账本，直接生效）：
  - `health` 携带 `protocolVersion`；runtime 首次连接比对，不符 → 每次调用返回 `version_mismatch` 错误结果。
  - `launch_app` 返回 Promise，调用处必须 `await`。
  - `maxElements` 缺省省略键（napi 拒绝显式 null）——backend 已处理，runtime 透传即可。
  - stop 后的变更类调用 → `controller_busy`（message 明示 computer control was stopped）。
  - 帧 ≤200KiB base64：生产侧归 Task 6（addon 阶梯）；`preserveOfficialCuaFrameResult` 为兜底闸门。
  - spec parked 行清理（L292 集成步骤 token 字样、L317 开放点标记已解决）归本计划 Task 8。

## 文件地图（先读再写）

| 文件 | 职责 | 状态 |
| --- | --- | --- |
| `packages/mode-cua/frame-contract.js` (+`.d.ts`) | 六谓词/解析器 + ref 文本与 integrity meta **构建器**（新增导出） | stub → 本计划实现 |
| `packages/mode-cua/index.js` (+`.d.ts` 既有) | `createComputerUseRuntime`：会话/观察/动作/收据/帧 | stub → 本计划实现 |
| `packages/mode-cua/broker.d.ts` | `HelperHealth` 追加 `protocolVersion?: string` | 追加一行 |
| `packages/mode-cua/broker-server.js` (+`.d.ts`) | resolver 关键面（见 Task 5 清单）；`HELPER_PROTOCOL_VERSION` 已在 | 部分 stub → Task 5 |
| `crates/mode-cua-ax/src/capture.rs` | ≤200KiB PNG→JPEG 质量阶梯 | Task 6 |
| `packages/mode-cua/test/*.test.mjs` | 新增 frame-contract / runtime-observe / runtime-action / not-ready 用例 | 新建 |
| `packages/services/test/cuaRuntimeDisplay.test.ts` | display 黄金（落点见 Task 7 决策规则） | 新建或改道 |

---

### Task 1: 帧契约六函数 + 构建器 + `protocolVersion` 补型

**Files:**
- Modify: `packages/mode-cua/frame-contract.js`（替换 stub 实现）
- Modify: `packages/mode-cua/frame-contract.d.ts`（追加两个构建器导出声明）
- Modify: `packages/mode-cua/broker.d.ts`（`HelperHealth` 加 `protocolVersion?: string;`）
- Test: `packages/mode-cua/test/frame-contract.test.mjs`（新建）

**Interfaces:**
- Produces（Task 3 直接消费）:
  - `buildOfficialCuaImageRefText(opts: {frameId: string, rasterSha256: string, width: number, height: number, mimeType: string}): string` — 返回**单行 JSON 文本**：`{"image_ref":{"frame_id":…,"credential":<32hex 每帧随机>,"raster_sha256":…,"width":…,"height":…,"mimeType":…}}`
  - `buildOfficialCuaFrameIntegrityMeta(opts: {frameId, rasterSha256}): {v: 1, frame_id, raster_sha256}` — 放 `_meta["mode.cua/official-frame-integrity-v1"]`（真值即可，消费方只查存在性）
  - 既有六函数（签名以 `frame-contract.d.ts` 为准）：`isOfficialCuaImageRefText` / `containsOfficialCuaImageRefCredentialText` / `parseOfficialCuaImageRef` / `readRasterEnvelopeIdentity`（= sha256(base64 解码) hex）/ `findOfficialCuaFrameContentPair`（相邻对，返回 `{image,imageRef,imageIndex,imageRefIndex}`）/ `preserveOfficialCuaFrameResult`（精确栅格闸门）/ `attestOfficialCuaFrameContent` / `containsImageRefAuthority`
- Consumes: 无（纯函数层）。`preserveOfficialCuaFrameResult(result, {imageProcessorPort, signal})` 的压缩端口语义读 `apps/mode-cli/packages/core/src/mcp/image-normalization.ts:41-44` 调用点后对齐。

- [ ] **Step 1: 写失败测试**

```js
// packages/mode-cua/test/frame-contract.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import {
  buildOfficialCuaImageRefText, buildOfficialCuaFrameIntegrityMeta,
  isOfficialCuaImageRefText, containsOfficialCuaImageRefCredentialText,
  parseOfficialCuaImageRef, readRasterEnvelopeIdentity,
  findOfficialCuaFrameContentPair, preserveOfficialCuaFrameResult,
  attestOfficialCuaFrameContent, containsImageRefAuthority,
} from "../frame-contract.js";

const png = (n) => Buffer.concat([Buffer.from([0x89,0x50,0x4e,0x47]), Buffer.alloc(n, 7)]);
const b64 = (buf) => buf.toString("base64");
const digest = (buf) => createHash("sha256").update(buf).digest("hex");

function pair(buf) {
  return [
    { type: "image", data: b64(buf), mimeType: "image/png" },
    { type: "text", text: buildOfficialCuaImageRefText({
      frameId: randomBytes(8).toString("hex"), rasterSha256: digest(buf),
      width: 4, height: 4, mimeType: "image/png" }) },
  ];
}

test("ref 文本整块识别 + credential/解析", () => {
  const [img, ref] = pair(png(64));
  assert.equal(isOfficialCuaImageRefText(ref.text), true);
  assert.equal(containsOfficialCuaImageRefCredentialText(ref.text), true);
  const parsed = parseOfficialCuaImageRef(ref.text);
  assert.equal(parsed.mimeType, "image/png");
  assert.equal(isOfficialCuaImageRefText("prefix " + ref.text), false);   // 只认整块
  assert.equal(isOfficialCuaImageRefText('{"image_ref":{}}'), false);     // 缺字段
  assert.equal(containsOfficialCuaImageRefCredentialText('{"image_ref":{"frame_id":"x"}}'), false);
});

test("digest 与配对", () => {
  const buf = png(64); const [img, ref] = pair(buf);
  assert.equal(readRasterEnvelopeIdentity(img), digest(buf));
  const content = [img, ref, { type: "text", text: "tree…" }];
  const found = findOfficialCuaFrameContentPair(content);
  assert.equal(found.imageIndex, 0); assert.equal(found.imageRefIndex, 1);
  assert.equal(findOfficialCuaFrameContentPair([img]), undefined);
  // 两对 → preserve 拒绝（keepLatest 在 host 已去重；此处锁 fail-closed）
});

test("attest：对完好对返回真值，缺 meta/digest 不符 → falsy", () => {
  const buf = png(64); const [img, ref] = pair(buf);
  assert.ok(attestOfficialCuaFrameContent([img, ref], "official_cua_frame_v1"));
  assert.equal(attestOfficialCuaFrameContent([{ type: "text", text: "x" }], "official_cua_frame_v1"), undefined);
});

test("preserve：digest 不符 → 整帧 fail-closed 替换", async () => {
  const buf = png(64); const [img, ref] = pair(buf);
  const tampered = { ...img, data: b64(png(65)) };
  const out = await preserveOfficialCuaFrameResult(
    { content: [tampered, ref], _meta: { "mode.cua/official-frame-integrity-v1": { v: 1 } } },
    { imageProcessorPort: undefined, signal: undefined });
  assert.equal(out.content.some((b) => b.type === "image"), false);
  assert.match(out.content[0].text, /not visible|不可见/u);
});

test("preserve：>200KiB 走压缩端口，压不进 → 降级不可见提示", async () => {
  const big = png(300 * 1024);
  const meta = { "mode.cua/official-frame-integrity-v1": { v: 1, frame_id: "f", raster_sha256: digest(big) } };
  const [img, ref] = pair(big);
  const compress = async () => ({ data: b64(png(100 * 1024)), mimeType: "image/jpeg" });
  const ok = await preserveOfficialCuaFrameResult({ content: [img, ref], _meta: meta },
    { imageProcessorPort: { compress }, signal: undefined });
  assert.equal(ok.content[0].type, "image");
  const fail = await preserveOfficialCuaFrameResult({ content: [img, ref], _meta: meta },
    { imageProcessorPort: { compress: async () => undefined }, signal: undefined });
  assert.equal(fail.content.some((b) => b.type === "image"), false);
});
```

（`imageProcessorPort` 的真实接口名以 `image-normalization.ts` 调用点读出的实际形状为准——Step 1 先读该文件再落测试，压缩端口字段名错了先改测试。）

- [ ] **Step 2: 跑测试确认失败** — Run: `pnpm --filter @mode/cua test` → Expected: FAIL（stub 返回 false/undefined）
- [ ] **Step 3: 实现** — 六函数 + 两个构建器（ref 单行 JSON、credential=`randomBytes(16).toString("hex")`；digest 统一 `sha256` hex 小写；`preserve` 逻辑：authority 路径由调用方保证，本函数校验 digest→压缩→降级三级，降级文案与 `official-cua-media.ts` 的 `OFFICIAL_CUA_RASTER_UNAVAILABLE_TEXT` **字面一致**（mode-cua 不得 import core，文案复制+注释来源）；`attest` = image@0+ref@1+digest 匹配 → 返回 truthy 包对象）。
- [ ] **Step 4: 跑全套** — `pnpm --filter @mode/cua test`（13 既有 + 新增全绿）、`pnpm typecheck`、`pnpm lint`
- [ ] **Step 5: Commit** — `feat(cua): 帧契约六函数与帧构建器真实现`

---

### Task 2: runtime 骨架——会话、收据、错误装配、CUA_NOT_READY、版本比对

**Files:**
- Modify: `packages/mode-cua/index.js`（`createComputerUseRuntime` 真实现；`ComputerUseRuntime` 签名以 `index.d.ts` 为准）
- Test: `packages/mode-cua/test/runtime-core.test.mjs`（新建）

**Interfaces:**
- Consumes: Task 1 构建器；`broker.js` `callBrokerMethod/HELPER_PROTOCOL_VERSION/BrokerError`；helper backend 八方法（Plan A Task 8：`health,list_apps,list_windows,observe,capture,perform,launch_app,screen_probe`）。
- Produces（Task 3/4/7 依赖，也是 host 消费面）:
  - `createComputerUseRuntime({brokerSocketPath, refreshMarkerPath, ensureBrokerAvailable, env}) → {execute, closeSession, dispose}`
  - `execute({toolName, arguments, context, signal})` 返回 MCP 结果，三种形态：
    1. 成功观察/动作：`{content, structuredContent, _meta?, isError:false}`
    2. 失败：`{isError:true, content:[{type:"text", text: JSON.stringify({code, message, suggested_action})}], structuredContent:{error:{code, suggested_action}}}`
    3. 冷启动未就绪：**非 error** 单文本块 `JSON.stringify({kind:"CUA_NOT_READY", reasonCode:"broker_not_accepting", retryable, message})`
  - 会话键：`` `${context.session_id}|${context.workspace_key ?? context.workspacePath ?? ""}` ``；会话持有：`observations`（每 appRef 一张）、`stopped`、`leaseOwner`、`lastFrame`、`versionSticky`（版本失配粘滞）。
  - 工具名集合 = SDK `COMPUTER_METHOD_NAMES` 14 个；未知 → `method_not_found` 错误结果。
  - 错误装配规则（**规范级，测试逐字锁**）：`BrokerError.code` ∈17 键 → 直接用；`suggested_action` 由映射表给出（`element_unavailable`→"Re-observe with get_app_state before acting again."、`foreground_required`→"Target the element index instead, or bring the app to the foreground first."、`controller_busy`→stop/他人持有 文案、其余→省略）。
  - 版本比对：runtime 首次 execute 前 `callBrokerMethod health`；`protocolVersion !== HELPER_PROTOCOL_VERSION` → 本 runtime 后续一切调用返回 `version_mismatch`（粘滞）；health 连接失败 → 视为冷启动（见下）。
  - 连接类失败（`ECONNREFUSED/ENOENT/stale_socket` 且从未成功过 health）→ `CUA_NOT_READY` 信封（`retryable:true`）；已成功过又断 → 按普通错误（`stale_socket`→SDK `HELPER_UNAVAILABLE`）。

- [ ] **Step 1: 写失败测试**（fake broker：`net.createServer` 在 `mintBrokerSocketPath()` 上按 method 回 canned JSON，用例覆盖：未知工具、17 码错误文本形态（`{code,message,suggested_action}` 双落点）、health 版本不符粘滞、无监听 → CUA_NOT_READY、`stop_computer_control` 后变更类 → `controller_busy`、会话键隔离（两个 context 互不共享 stopped）、`dispose` 后调用 → `broker_unavailable`）
- [ ] **Step 2: 跑测试确认失败** — Expected: FAIL（stub 返回 unavailable 文本）
- [ ] **Step 3: 实现** — 分发表预留 `observe/action` 处理器钩子（Task 3/4 注册）；本任务先接 `list_apps/list_windows/health/request_access/stop_computer_control` 五个直通工具 + 全部基础设施。
  - `list_apps`：broker 数组 → **单文本块 = 裸 JSON 数组文本**（SDK `parseJsonValue` 明确处理裸数组）。
  - `request_access`（Windows）：不打 broker，直接合成——文本块=扁平 `AccessStatus` `{"ready":true,"accessibility":"granted","screenRecording":"granted"}`；`structuredContent={"platform":"windows","backend":"uia","accessibility":{"status_after":"not_required"},"screen_recording":{"status_after":"not_required"}}`；**不设** darwin-only meta。
  - `stop_computer_control`：置 `stopped=true`、释放 lease、返回收据成功（可重复调用）；此会话后续变更类工具（除 `get_app_state/list_*/request_access`）→ `controller_busy`。
- [ ] **Step 4: 跑测试** — `pnpm --filter @mode/cua test` 全绿
- [ ] **Step 5: Commit** — `feat(cua): runtime 骨架与直通工具`

---

### Task 3: 观察面——`get_app_state` 渲染、diff、位移台账、帧对

**Files:**
- Modify: `packages/mode-cua/index.js`（注册 observe 处理器）
- Test: `packages/mode-cua/test/runtime-observe.test.mjs`（新建）

**Interfaces:**
- Consumes: Task 1/2 全部；broker `observe({windowId,maxElements?})`/`capture({windowId?,region?})` 返回（形状见 Plan A addon Produces）。
- Produces:
  - `get_app_state(app_ref, include_screenshot=false, disable_diffing=false, tree_shown_to_model=true)` —— **SDK 只会发这四个键**（strict schema；见 client `observe()` 与 `bindApp` 的实参构造，实现前先读 `computer-use-client.mjs` 约 575-660、965-1000 行把实参形状抄准）。
  - 窗口解析：`app_ref.window_id` 缺省时 `list_windows(pid)` 取 `main||focused` 行（docs 语义：主/键窗口，每次观察重新解析）。
  - **内容顺序（规范级）**：
    - 纯截图观察（`include_screenshot=true && tree_shown_to_model=false`）：`content=[image@0, ref@1]`，无树文本。
    - 带树观察：`content=[treeText]`；若同时 `include_screenshot=true`：`content=[image@0, ref@1, treeText@2]`（帧对必须 0/1，投影层强校验）。
    - 树文本**始终**是单个 text 块，且它是 non-JSON 文本（advisory 过滤依赖此性质）。
  - **树文本格式**（UI `cuaResultState.ts` 逐字解析，测试用同款正则锁）：
    - 首行：`app: <sanitized> pid=<N> "<title>"`——`sanitized` = exe/AUMID 显示名 sanitize 到 `[A-Za-z0-9.-]+`（结构化 `app.name` 保留原名）。
    - 元素行：`[<idx>] <kind> <title>[ = <value>][ (<annotation>)]`——UI 规则：末尾 `(...)` 会被剥、最后一个 `" = "` 分词、数值型右侧取左名/文本型取右名。`idx` 必须= broker observe 返回的 index（**原样透传，不重编号**）。
    - 行内附该元素 `actions`（如 `[3] button OK = null (press)`）；`offscreen` 元素行尾 ` (offscreen)`。
    - 提示行：diff 头 `changes: +N -N ~N`、`indices are sparse`（broker 元素数被 maxElements 截断且 `enumerationComplete=false` 时）、`showing A-B of N items`（元素数达到 3000 上限）、`[effect_evidence unchanged]`（动作后观察且树未变——由动作处理器注入，见 Task 4）。
  - **structuredContent**（`appStateOf`/UI 双消费，字段全集）：`state_id`（`randomUUID`/观察序）、`base_state_id`、`snapshot_mode: "full"|"diff"`、`app:{name,bundle_id,pid,owner?}`、`window:{title,window_id}`、`focused_element: number|null`、`elements: [{index,kind,title,value,actions,bounds,enabled,offscreen}]`、`changes?: {added_count,removed_count,changed_count,added:[],removed:[],changed:[]}`。
  - **diff 语义**：基线=该会话该 appRef **最近一次 `tree_shown_to_model!==false` 的观察**（SDK 注释的「模型看过的树」）；`disable_diffing=true` 或基线不存在 → 全量；纯截图观察**不**成为基线但**使**下一次带树观察强制全量（docs 规则）；diff = 按 `index→fingerprint(kind,title,value,bounds[0..3])` 的 added/removed/changed（index 对不上但 fingerprint 匹配 → changed 位移）。
  - **位移台账（headline 正确性，两条规则+测试）**：
    1. 观察记录带 `shownToModel` 标志；隐藏观察（`tree_shown_to_model=false`）**更新**元素表与 Rust 侧缓存，但**不**成为 diff/displacement 基线。
    2. 动作带 `elementIndex` 时（Task 4）：若最后一次观察是隐藏的且其元素序列与最近 shown 基线的序列**不一致**（位移/增删）→ 拒绝 `element_unavailable`（提示 re-observe），绝不拿模型没见过的编号去解析——这是 SDK 注释「静默点错元素的保护」的 producer 侧落点。
  - **帧**：`include_screenshot=true` → `capture`；返回 base64 > 200KiB → `internal` 错误（Task 6 阶梯落地后此分支应不可达，保留为防御）；`_meta` 写 integrity meta + `mode.cua/app-associations-v1 = {primary:{appKey, displayName}}`（`appKey = bundle_id ?? name ?? String(pid)`，16KiB 内）。
  - 收据进 `structuredContent` 顶层：`state_id/frame_id/snapshot_mode/base_state_id`（SDK `receiptOf` 合并读取）。
- [ ] **Step 1: 先读** SDK `observe()`/`bindApp`/`appStateOf` 实际读取的字段（报告里列出你钉到的字段清单），再写失败测试（fake broker canned observe/capture 响应 → 断言渲染树正则、帧对顺序、meta、diff 计数、隐藏基线规则、截图-only 无树、>200KiB 防御错误）
- [ ] **Step 2: 确认失败** → **Step 3: 实现** → **Step 4: 全绿**（含既有套件）
- [ ] **Step 5: Commit** — `feat(cua): 观察渲染、diff 与位移台账`

---

### Task 4: 动作面——10 个变更工具 + 目标解析 + 收据三态

**Files:**
- Modify: `packages/mode-cua/index.js`（注册 action 处理器）
- Test: `packages/mode-cua/test/runtime-action.test.mjs`（新建）

**Interfaces:**
- Consumes: Task 2/3；broker `perform({kind,windowId,payload})`。
- **载荷键名（已对照 Rust `parse_payload` 钉死，不得发明）**：

| SDK 工具 | broker kind | payload（runtime 构造） |
| --- | --- | --- |
| `left_click({target,mouse_button,click_count,modifiers,strategy,app_ref,return_state})` | `click` | `{x,y}`（element 目标→其 bounds 中心）+ `{button:"left"..., clickCount, modifiers}` |
| `left_click_drag({from_target,to,modifiers,...})` | `click_drag` | `{fromX,fromY,toX,toY,modifiers}` |
| `scroll({target,scroll_direction,scroll_amount,...})` | `scroll` | `{x,y,direction,amount}`（amount 透传，Rust clamp 0-100） |
| `type({text,target?,...})` | `type_text` | `{text}`；`target` 给了先 `click` 聚焦（复合调用顺序：click→type_text，两次 perform，收据合并 action_sent=OR） |
| `set_value({target,value,strategy,...})` | `set_value` | `{elementIndex, value}`（坐标目标→`{x,y,value}` 聚焦点） |
| `select_text({target,text_range,...})` | `select_text` | `{elementIndex, start, length}`（无 text_range → `{start:0,length:-1}`？**先读 Rust 侧对负 length 的语义**，不行就取观察到的 value 长度） |
| `key({text,repeat,hold_seconds,...})` | `key` | `{chord, repeat, holdMs: hold_seconds*1000}` |
| `paste({text,format?})` | `paste` | `{text}` |
| `perform_action({target,action,...})` | `action` | `{elementIndex, action}` |
| `stop_computer_control` | （Task 2 已实现） | — |

  - **目标解析规则**：SDK `bindTarget` 发 `{type:"element",index}` 或 `{type:"coordinate",x,y,frame_id?}`。
    - element：先过**位移台账门**（Task 3 规则 2）→ 命中最近观察的 `elements[index]`，不存在/窗口已换 → `element_unavailable`；坐标= bounds 中心（floor）。
    - coordinate：`frame_id` 给了 → 与会话 `lastFrame` 比对，不符/过期/owner 不符 → `invalid_request`（message 含 `frame_dispatch_identity_mismatch` 语义）；没给帧 → 最近可动作帧兜底，无帧 → `invalid_request`（message 含 "no actionable frame is available in this transport" 语义）；坐标超出该帧尺寸 → `invalid_request`。
    - `strategy:"event"` 语义由 Rust 前台门承担，runtime 不重复判定，只透传 strategy？—— **Rust payload 无 strategy 键**（对照 parse_payload 已确认）→ strategy 一期忽略（docs：event/a11y 分支由 Rust 内部 auto 决策），在报告记录该偏差。
  - **收据装配（成功）**：`structuredContent` 含 `action_outcome:{action_sent, dispatch_status}`——Rust `dispatched` 映射：`dispatched→{action_sent:true,dispatch_status:"delivered"}`、`not_dispatched→{action_sent:false,dispatch_status:"not_sent"}`、`unknown→{action_sent:true,dispatch_status:"possibly_sent"}`；失败（isError）且 dispatched==="unknown" → 同样带 `dispatch_status:"possibly_sent"`。
  - **失败形态**：Task 2 的错误装配（文本 JSON `{code,message,suggested_action}` + `structuredContent.error`）。
  - **成功动作带 `state_sync_status`**：动作后我们不知道 UI 是否变化（docs：SDK 不清 stateId）→ 固定 `state_sync_status:"unconfirmed"`（SDK 只读不校验，报告注明）。
  - `return_state:"compact|full"`：动作后附带观察（复用 Task 3 渲染，structuredContent 并入），默认 `none` 不观察。
  - **`[effect_evidence unchanged]`**：`return_state` 观察且 diff 为空 → 树尾注该行。
- [ ] **Step 1: 先读** `computer-use-client.mjs` 的 action 调用处（`act()` 各工具实参，约 660-860 行）+ Rust `parse_payload`（`crates/mode-cua-ax/src/perform.rs:688-775`）—— 把两侧键名对照表抄进报告；select_text 无 range 的 Rust 语义查 Rust `SelectText` 处理代码后定桩。
- [ ] **Step 2: 写失败测试**（fake broker：element 解析中心点、位移门拒绝、坐标帧校验三分支、三态收据映射、复合 type、return_state 附观察、stopped 后 controller_busy）
- [ ] **Step 3: 确认失败 → 实现 → 全绿**
- [ ] **Step 4: Commit** — `feat(cua): 动作工具面与收据三态`

---

### Task 5: resolver 关键面——拉起 host、注入凭据、mac 保持 fail-closed

**Files:**
- Modify: `packages/mode-cua/broker-server.js` + `packages/mode-cua/broker-server.d.ts`（仅追加实现/声明，不改既有 stub 语义与 `HELPER_PROTOCOL_VERSION`）
- Test: `packages/mode-cua/test/resolver.test.mjs` + 视消费方测试落点（services 侧已有注入链测试则补 services 用例）

**Interfaces:**
- Consumes（**实现前必须逐个读，报告贴出调用点与所需语义**）:
  - `packages/services/src/node.ts`（工厂 ~894-1095：`createCuaProductMcpServerResolver(host,{hasActiveTurn})` 的用法、`createWindowsCuaHelperHost` 已真实现）
  - `apps/mode-cli/packages/bootstrap/src/mcp-config.ts`（`injectCuaCredentialsIntoNodeRepl`、`getCapturedModeCuaBrokerCredentials` 消费位——凭据从**进程 env 捕获**，故 resolver 的职责是让 socket 出现在 bootstrap 进程 env 或走 `buildCuaProductHelperAgentEnv` 注入路径——读 `node.ts:1220-1290` 的 `buildCuaProductHelperAgentEnv` 与 `modeSessionService.ts:183`/`modeTaskServiceAdapter.ts:340` 的调用条件后，按**真实链**实现）
  - `packages/shared/src/runtimeEnv.ts`（捕获/清洗语义、`MODE_CUA_PERMISSION_BROKER_UNAVAILABLE` 的读方）
- Produces（最小真实现集，**win32 链必需**）:
  - `createCuaProductMcpServerResolver(host, {hasActiveTurn}) → {resolveMcpServers(servers, ctx), restart(), restartAfterPermissionGrant(id)}`：
    - `resolveMcpServers`：官方 CUA 插件启用且 win32 → `host.start()`（失败 → 记 unavailable 标记 + 原样返回 servers，**不抛**）→ 保证凭据进注入链 → 返回 servers；插件未启用 → 原样返回且**不** start。
    - `restart`：`host.restart()`；`restartAfterPermissionGrant`：Windows 直接 `return`（无 TCC）。
  - `isOfficialCuaPluginEnabledForWorkspace({env, workingDirectory})`：按消费方实际读取的 env/配置位实现（读出什么实现什么；若它实际读 `enabledPlugins` env 快照——以消费方为准）。
  - `markCuaProductHelperAgentEnvUnavailable / has… / clear…`：进程内标志位（unavailable 后 `resolveMcpServers` 短路直返，直到显式 clear）。
  - `waitForCuaHelperStartup(promise, deadline)`：真超时包装（现有 stub 是恒等透传——按 d.ts 语义实现 deadline → reject `broker_unavailable`）。
  - `isScreenCaptureProbeSuccess` / `isPotentialModeCuaAgentMcpServer` / `reapOrphanedHelpers` / refresh-marker 两个函数：**读消费方后**逐一决定真实现或保持 stub（mac-only 的保持 stub 并在报告分类）；每个函数给出「消费方是谁、真实现/保留 stub 的理由」。
- [ ] **Step 1: 消费方追踪**（上列文件 + `rg` 找全部调用点，输出「stub 函数 → 消费方 → 决定」表）
- [ ] **Step 2: 写失败测试**（fake host 注入：启用→start 被调且凭据可见；未启用→不 start；start 失败→短路+标记；clear 后恢复；restartAfterPermissionGrant Windows no-op；mac 分支函数保持 stub 恒值）
- [ ] **Step 3: 确认失败 → 实现 → 全绿**（services 既有 218 测试不得回归）
- [ ] **Step 4: Commit** — `feat(cua): resolver 凭据链真实现（mac 保持 fail-closed）`

---

### Task 6: Rust 截图 ≤200KiB 质量阶梯（小改，独立可测）

**Files:**
- Modify: `crates/mode-cua-ax/src/capture.rs`
- Test: `crates/mode-cua-ax/tests/capture.rs`（追加）

**Interfaces:**
- Consumes: Task 4 既有 PNG 编码路径。
- Produces: `capture` 返回前——若 base64 长度（`bytes.len()*4/3` 估算即可，实现按 `base64_len` 精确算）> `200*1024` → 以 JPEG 质量阶梯 `[85,70,55,40]` 重编码；全部仍超 → `AxError::new("internal", "capture exceeds frame budget")`；`mimeType` 随结果变 `image/jpeg`。
- **Toolchain prelude（每个 shell）**:
  ```bash
  export PATH="$HOME/.rust-tools/llvm-mingw-20261006-ucrt-x86_64/bin:$HOME/.cargo/bin:$PATH"
  export CARGO_BUILD_TARGET=x86_64-pc-windows-gnu
  export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$HOME/.rust-tools/llvm-mingw-20261006-ucrt-x86_64/bin/x86_64-w64-mingw32-clang"
  export LIBNODE_PATH="$HOME/.rust-tools/libnode-stub"
  cargo +stable-x86_64-pc-windows-gnu <test|clippy> --manifest-path crates/mode-cua-ax/Cargo.toml ...
  ```
- [ ] **Step 1: 失败测试**——阶梯函数抽 `pub(crate)` 纯函数（合成大缓冲：噪声 RGB → png 编码 >200KiB → 断言阶梯输出 ≤200KiB 且 mimeType=jpeg；恒超输入 → internal 错误）+ 既有用例全绿
- [ ] **Step 2-4: 实现 → 全绿（cargo test 40+ 新增、clippy `-D warnings`）→ `pnpm build:cua-helper` 重建产物**
- [ ] **Step 5: Commit** — `feat(cua-ax): 截图帧预算质量阶梯`

---

### Task 7: display 黄金 + 真链路集成扩展

**Files:**
- Create: `packages/services/test/cuaRuntimeDisplay.test.ts`（落点决策见下）
- Modify: `packages/mode-cua/test/integration.helper.test.mjs`（扩展 runtime 级断言，或新建 `integration.runtime.test.mjs`——二选一，报告说明）

**Interfaces:**
- Consumes: Task 1-6；core `createCuaToolResultDisplay`（`apps/mode-cli/packages/core/src/tool/executor/result-display.ts`）；shared `toolResultDisplaySchema`。
- **落点决策规则（先探测再写）**：`packages/services` 能 `import "@mode/core"`（查 services package.json deps/paths 与既有测试先例）→ 黄金测试放 services：canned runtime observe/动作结果 → `createCuaToolResultDisplay(...)` → `toolResultDisplaySchema` 校验 + 断言 UI 关键字段（`kind:"cua"`、`state_id` 在 structuredContent、media dataUrl 可被 `cuaScreenshotDetails` 的正则识别、errorCode 映射）+ `isOfficialCuaFrameAuthority` 真值。不能 import → 改在 `packages/mode-cua/test/` 内做「帧对 + meta + 错误文本」的消费方契约替身（正则从 `cuaResultState.ts`/`image-normalization.ts` **复制并注明来源行号**），并把跨包黄金显式移交 Plan C 的 desktop E2E——报告写明走了哪条。
- **集成扩展**（`CUA_INTEGRATION=1`，真 helper+addon+notepad）：经 `createComputerUseRuntime().execute` 全链：
  1. `list_apps`（裸数组文本）→ 2. `get_app_state`（`{include_screenshot:true, tree_shown_to_model:false}` 纯截图 → 断言 `content[0].type==="image"`、`isOfficialCuaImageRefText(content[1].text)`、`_meta` integrity 真值、base64 ≤200KiB、无树文本）→ 3. `get_app_state` 带树（首行 header 正则、元素行）→ 4. `left_click` element 目标 + `type_text` → 5. 重观察读回 → 6. `request_access` 形状 → 7. `stop_computer_control` → 变更类 `controller_busy` → 8. 杀 helper → 新调用得 `CUA_NOT_READY` 信封（或 stale_socket，按 runtime 冷热判定断言到其一并注明）→ 9. fake-broker 版 `version_mismatch` 单测在 runtime-core 已覆盖。
- [ ] Steps: 探测落点 → 真失败测试 → 实现/扩展 → `CUA_INTEGRATION=1 …test:integration` PASS + `pnpm --filter @mode/services test` 全绿
- [ ] **Commit** — `test(cua): display 黄金与 runtime 端到端集成扩展`

---

### Task 8: 全仓验证 + spec/plan 文档同步（收尾）

**Files:**
- Modify: `docs/specs/computer-use-windows-runtime.md`（stub 收敛清单、parked 两行清理）
- Modify: 本计划文件（勾选与偏差记录）
- Test: 无新测试；跑全部

- [ ] **Step 1: 文档同步**：
  1. spec L292 集成步骤「token 鉴权」改为「无口令直连（路径即凭据）」；L317 开放点标记「已解决：Plan A 裁决=连接无口令」（行号以当前 spec 为准，先 grep `token` 定位）。
  2. spec 增补「stub 收敛状态」小节：列出 Plan B 后仍为 stub 的函数（mac-only 族 + Plan A 保留族），逐个一句话理由。
  3. 本计划 Global Constraints/任务标注实际偏差（各任务报告汇总）。
- [ ] **Step 2: 全仓验证（真实结果，不注水）**：
  - `pnpm --filter @mode/cua test`、`pnpm --filter @mode/services test`、`CUA_INTEGRATION=1 pnpm --filter @mode/cua run test:integration`
  - `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`
  - `cargo +stable-x86_64-pc-windows-gnu test` + `clippy --all-targets -- -D warnings`
  - `node scripts/check-language-policy.mjs`——**预期仍红**（Plan A 的 8 处 + 本计划新 `.mjs` 增量如实计数；allowlist 归 Plan C/并行会话，裁决不变）
- [ ] **Step 3: Commit** — `docs(cua): Plan B 收尾与 spec stub 收敛`

---

## 后续计划（不在本文件范围）

- **Plan C — 发布**：electron-builder `tools/cua-helper` extraResources、`MODE_CUA_AX_TARGET` CI 注入、语言政策 allowlist 8+行、NOTICE/README 文案（占位声明移除）、desktop E2E、实机验收清单、`__MODE_CUA_HELPER_BUILD_ID__` 处置。
- **Plan B 已知 carry 给 C**：集成测试 timeout 240s 若加 `perform` 整体 deadline 需重估；跨包 display 黄金若在 Task 7 移交则 E2E 必须覆盖。
