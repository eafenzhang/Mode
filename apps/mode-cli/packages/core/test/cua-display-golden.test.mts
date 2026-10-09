/* eslint-disable max-lines -- display 真黄金：六个 canned runtime 形状平铺 + 逐处来源注释，
   与 packages/mode-cua/test/display-contract.test.mjs 同款处置（副本/注释占多数）。 */
// display 真黄金（Plan C Task 4）——真消费方 core 本尊对 canned runtime 出参的跨包 display 黄金。
//
// 与 Plan B T7 替身（packages/mode-cua/test/display-contract.test.mjs）的分工：那份是
// 「services 不能 import @mode/core」降级出来的**逐行等价副本**（包内契约锁，保留双锁）；
// 本文件 import 的是 core 真身 dist（result-display.ts / image-normalization.ts），因此能覆盖
// 副本显式未复制的分支：32KiB bound + truncated（result-display.ts:233、267-268、302-303、329）——
// 即 Plan B T7 报告里欠下的账。
//
// 落点与运行前提（证据见 task-4-report.md §Step 1）：
//   - mode-cli 各包此前**没有任何 test 脚本**；release.yml verify 只跑 shared/services/server
//     + desktop tests，不收 mode-cli 包测试 → 本任务只落地本地脚本，CI 接线移交收尾。
//   - core 的 dist/ 被 gitignore，且 dist JS 会 import "@mode/shared"（其 exports 指向 .ts 源，
//     相对导入写 .js 后缀）→ 纯 `node --test` 解析不了 .js→.ts 重映射（实测 ERR_MODULE_NOT_FOUND），
//     沿用仓库既有 `tsx --test` 惯例（@mode/shared / @mode/services / @mode/server 同款）。
//   - 前提：先 build（dist 缺失即 import 失败）：pnpm --filter @mode/core build。
//   - 语言政策：新文件用 .mts（.js/.mjs/.cjs 在禁用扩展名清单内）。
//
// fixture 来源：形状逐键镜像 packages/mode-cua/index.js 的真实装配点（帧对 :760、观察
// structuredContent :790-812、动作收据 :1083-1098、errorResult :133-156、request_access
// :324-340、targetApp meta 由 contracts 生产方写入）。
import assert from "node:assert/strict";
import { createHash, randomUUID } from "node:crypto";
import test from "node:test";

import {
  CUA_TARGET_APP_DISPLAY_META_KEY,
  toolResultDisplayPayloadSchema,
} from "@mode/contracts";
import {
  OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY,
  OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES,
  buildOfficialCuaFrameIntegrityMeta,
  buildOfficialCuaImageRefText,
} from "@mode/cua/frame-contract";
import { CUA_REQUEST_ACCESS_STATUS_META_KEY } from "@mode/cua/request-access-contract";
import { toolOutputSchema } from "@mode/shared/mode-protocol-v4";

import { hasOfficialCuaFrameAuthority } from "../dist/mcp/image-normalization.js";
import { createToolResultDisplay } from "../dist/tool/executor/result-display.js";

// result-display.ts:233 —— 32KiB display 字段上限（boundDisplayText 的 maxBytes 口径）。
const MAX_CUA_DISPLAY_FIELD_BYTES = 32 * 1024;
// display-text.ts:2 —— 截断后缀（boundDisplayText 拼接的字面量，逐字断言）。
const DISPLAY_TRUNCATION_SUFFIX = "\n...[truncated]";
// ui cuaScreenshotDetails.ts:23 —— 字符串分支 dataUrl 发现正则（逐字复制）。
const UI_DATA_URL_RE = /^data:image\/[a-z0-9.+-]+;base64,/iu;
// ui cuaScreenshotDetails.ts:68 —— dataUrl 的 mime 捕获正则（逐字复制）。
const UI_DATA_MIME_RE = /^data:(image\/[a-z0-9.+-]+);base64,/iu;

type UnknownRecord = Record<string, unknown>;

function rec(value: unknown): UnknownRecord | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as UnknownRecord)
    : undefined;
}

function assertDisplayParses(display: unknown, label: string): void {
  assert.ok(rec(display), `${label}: display 必须是对象`);
  // ① CLI 侧 strict（core 返回类型 ToolResultDisplayPayload 的 schema 来源；
  //    contracts tool-result-metadata.ts:117-146 的 cua 成员 .strict() —— 未知键直接拒）。
  const cli = toolResultDisplayPayloadSchema.safeParse(display);
  assert.ok(
    cli.success,
    `${label}: contracts toolResultDisplayPayloadSchema 拒绝 — ${JSON.stringify(
      cli.success ? undefined : cli.error.issues,
    )}`,
  );
  assert.deepEqual(cli.data, display, `${label}: contracts 解析必须逐键等值`);
  // ② 协议侧（UI 收到的那道）：shared toolOutputSchema 的 display 成员
  //    = toolResultDisplaySchema（shared mode-protocol-v4/toolDisplay.ts:23 union、:157
  //    .optional().catch(undefined) —— 校验失败会**静默变 undefined**，所以要反向断言：
  //    parse 出来非 undefined 且与入参等值，才能证明没被拒、也没被 strip 掉键）。
  const wire = toolOutputSchema.parse({ text: "ok", display });
  assert.ok(wire.display !== undefined, `${label}: shared toolResultDisplaySchema 拒绝了 display`);
  assert.deepEqual(wire.display, display, `${label}: shared 解析必须逐键等值（防 strip）`);
}

// ────────────────────────────────────────────── canned runtime 出参（形状镜像 index.js 装配点）

// 64x32 小 PNG 字节（与 Plan B 替身同款）：够帧对/预算断言即可，不必真解码。
const PNG_BYTES = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47]), Buffer.alloc(60, 1)]);
// readRasterEnvelopeIdentity（frame-contract.js:125）= sha256(base64 解码字节) 的小写 hex。
const RASTER_SHA256 = createHash("sha256").update(PNG_BYTES).digest("hex");
const IMAGE_BASE64 = PNG_BYTES.toString("base64");

// 帧对：content[0]=image、content[1]=refText（index.js:760 push 顺序）+
// _meta[OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY]（index.js:810）。
function framePair() {
  const frameId = randomUUID();
  const refText = buildOfficialCuaImageRefText({
    frameId,
    rasterSha256: RASTER_SHA256,
    width: 64,
    height: 32,
    mimeType: "image/png",
  });
  const integrityMeta = buildOfficialCuaFrameIntegrityMeta({
    frameId,
    rasterSha256: RASTER_SHA256,
  });
  return { frameId, refText, integrityMeta };
}

const TREE_TEXT = 'app: notepad.exe pid=7 "Untitled"\n\n  [3] edit field "" value=""\n';

function observationOutput() {
  const { frameId, refText, integrityMeta } = framePair();
  const stateId = randomUUID();
  return {
    stateId,
    output: {
      isError: false,
      content: [
        { type: "image", data: IMAGE_BASE64, mimeType: "image/png" },
        { type: "text", text: refText },
        { type: "text", text: TREE_TEXT },
      ],
      // index.js:790-812 的观察 structuredContent（八键元素集同 Plan B 替身 el()）。
      structuredContent: {
        state_id: stateId,
        base_state_id: null,
        snapshot_mode: "full",
        app: { name: "notepad.exe", pid: 7 },
        window: { title: "Untitled", window_id: 99 },
        focused_element: null,
        elements: [
          {
            index: 3,
            kind: "edit",
            title: "field",
            value: "",
            bounds: [30, 0, 80, 24],
            actions: [],
            enabled: true,
            offscreen: false,
          },
        ],
        frame_id: frameId,
      },
      _meta: { [OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY]: integrityMeta },
    },
  };
}

// 动作成功收据（index.js:1083-1098：structuredContent = action_outcome + 同值顶层 + state_sync）。
const actionReceiptOutput = {
  isError: false,
  content: [
    {
      type: "text",
      text: JSON.stringify({
        action_sent: true,
        dispatch_status: "delivered",
        state_sync_status: "unconfirmed",
      }),
    },
  ],
  structuredContent: {
    action_outcome: { action_sent: true, dispatch_status: "delivered" },
    action_sent: true,
    dispatch_status: "delivered",
    state_sync_status: "unconfirmed",
  },
};

// 失败（index.js:133-156 errorResult：文本 JSON + structuredContent.error 双落点）。
const failureOutput = {
  isError: true,
  content: [
    {
      type: "text",
      text: JSON.stringify({
        code: "element_unavailable",
        message: "element 4 is gone",
        suggested_action: "Re-observe with get_app_state before acting again.",
      }),
    },
  ],
  structuredContent: {
    error: {
      code: "element_unavailable",
      suggested_action: "Re-observe with get_app_state before acting again.",
    },
  },
};

// request_access（index.js:324-340 合成：扁平 AccessStatus 文本 + platform 投影，无 _meta）。
const requestAccessOutput = {
  isError: false,
  content: [
    {
      type: "text",
      text: JSON.stringify({
        ready: true,
        accessibility: "granted",
        screenRecording: "granted",
      }),
    },
  ],
  structuredContent: {
    platform: "windows",
    backend: "uia",
    accessibility: { status_after: "not_required" },
    screen_recording: { status_after: "not_required" },
  },
};

// targetApp 合法 meta（contracts tool-result-metadata.ts:38-46 的 strict 形状）。
const VALID_TARGET_APP = {
  schemaVersion: 1,
  displayName: "Notepad",
  iconLocators: [{ kind: "windows-executable-path", value: "C:\\Windows\\System32\\notepad.exe" }],
};

// ────────────────────────────────────────────── 黄金

test("帧对观察 → display：strict 双 schema + media dataUrl 可被 UI 发现正则采集 + 帧权威真值", () => {
  const { output, stateId } = observationOutput();

  // 显示前的帧权威真值（core mcp/image-normalization.ts:92 hasOfficialCuaFrameAuthority 真身）。
  assert.equal(hasOfficialCuaFrameAuthority(output), true, "帧对观察必须具帧权威");

  const display = createToolResultDisplay("mcp__computer_use__get_app_state", output, {
    officialCua: true,
  });
  assertDisplayParses(display, "帧对观察");
  const d = display as UnknownRecord;
  assert.equal(d.kind, "cua");
  assert.equal(d.schemaVersion, 1);
  assert.equal(d.toolName, "get_app_state", "action 段归一化（result-display.ts:237 readCuaToolName）");
  assert.equal(d.status, "success");
  assert.equal("truncated" in d, false, "预算内的观察不得置 truncated");

  // structuredContent JSON 面（UI cuaResultState 走 display.structuredContent）。
  const structured = JSON.parse(d.structuredContent as string) as UnknownRecord;
  assert.equal(structured.state_id, stateId);
  assert.equal((structured.window as UnknownRecord).window_id, 99);
  assert.ok(Array.isArray(structured.elements));
  // 树文本必须进 display.text（UI 树解析的取材面）。
  assert.ok((d.text as string).includes(TREE_TEXT.trim()), "树文本进 display.text");

  // media 发现：display.media[0] = {mimeType, data}；UI 侧按 cuaScreenshotDetails.ts:41-42
  // 拼成 dataUrl（`data:${mimeType};base64,${data}`），再走 :23 字符串分支正则。
  const media = d.media as Array<{ mimeType: string; data: string }>;
  assert.equal(media.length, 1, "帧对观察恰一个内联媒体");
  const dataUrl = `data:${media[0].mimeType};base64,${media[0].data}`;
  assert.ok(UI_DATA_URL_RE.test(dataUrl), `cuaScreenshotDetails.ts:23 不匹配: ${dataUrl.slice(0, 40)}`);
  const mime = UI_DATA_MIME_RE.exec(dataUrl)?.[1];
  assert.equal(mime, media[0].mimeType, "cuaScreenshotDetails.ts:68 mime 捕获与 media.mimeType 同值");
  const bytes = Buffer.byteLength(media[0].data, "base64");
  assert.ok(bytes > 0 && bytes <= OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES, `帧 base64 ≤ 200KiB（实际 ${bytes}）`);
});

test("动作成功收据 → display：action_sent/dispatch_status 透传 + plugin 工具名归一化", () => {
  // 真实插件命名空间形态（result-display.ts:241-246：`-`→`_` 归一化后取最后一个 __ 之后的 action）。
  const display = createToolResultDisplay(
    "mcp__plugin_mode-cua_computer-use__left_click",
    actionReceiptOutput,
    { officialCua: true },
  );
  assertDisplayParses(display, "动作收据");
  const d = display as UnknownRecord;
  assert.equal(d.status, "success");
  assert.equal(d.toolName, "left_click");
  assert.equal(d.errorCode, undefined, "成功收据无 errorCode");
  assert.equal(d.suggestedAction, undefined);
  assert.equal("truncated" in d, false);
  const structured = JSON.parse(d.structuredContent as string) as UnknownRecord;
  assert.equal(structured.action_sent, true, "收据顶层 action_sent 透传");
  assert.equal(structured.dispatch_status, "delivered");
  assert.equal(
    (structured.action_outcome as UnknownRecord).action_sent,
    true,
    "SDK receiptOf 读的嵌套 action_outcome 同值",
  );
});

test("失败 → display：status failed + errorCode/suggestedAction 双落点透传", () => {
  const display = createToolResultDisplay("mcp__computer_use__left_click", failureOutput, {
    officialCua: true,
  });
  assertDisplayParses(display, "失败动作");
  const d = display as UnknownRecord;
  assert.equal(d.status, "failed", "isError → status failed（result-display.ts:319）");
  assert.equal(d.errorCode, "element_unavailable");
  assert.equal(
    d.suggestedAction,
    "Re-observe with get_app_state before acting again.",
    "suggested_action → suggestedAction 透传（result-display.ts:323-325）",
  );
  assert.equal(d.media, undefined, "错误结果无媒体");
});

test("超 32KiB → display.text/structuredContent 有界 + truncated（替身未复制的欠账分支）", () => {
  // result-display.ts:233 MAX_CUA_DISPLAY_FIELD_BYTES = 32 * 1024；
  // :267-268 structured 与 text 都过 boundDisplayText；:302-303/:329 合成 truncated。
  const oversizedText = "x".repeat(40 * 1024);
  const oversizedStructured = { blob: "y".repeat(40 * 1024) };
  const display = createToolResultDisplay(
    "mcp__computer_use__list_windows",
    {
      isError: false,
      content: [{ type: "text", text: oversizedText }],
      structuredContent: oversizedStructured,
    },
    { officialCua: true },
  );
  assertDisplayParses(display, "截断");
  const d = display as UnknownRecord;
  assert.equal(d.truncated, true, "任一字段越界 → truncated");

  const boundedText = d.text as string;
  const boundedStructured = d.structuredContent as string;
  assert.ok(
    Buffer.byteLength(boundedText, "utf8") <= MAX_CUA_DISPLAY_FIELD_BYTES,
    `text 必须 ≤ 32KiB（实际 ${Buffer.byteLength(boundedText, "utf8")}）`,
  );
  assert.ok(
    Buffer.byteLength(boundedStructured, "utf8") <= MAX_CUA_DISPLAY_FIELD_BYTES,
    `structuredContent 必须 ≤ 32KiB（实际 ${Buffer.byteLength(boundedStructured, "utf8")}）`,
  );
  assert.ok(boundedText.endsWith(DISPLAY_TRUNCATION_SUFFIX), "text 带截断后缀（display-text.ts:2 字面量）");
  assert.ok(boundedStructured.endsWith(DISPLAY_TRUNCATION_SUFFIX), "structuredContent 带截断后缀");
  // 有界但仍是前缀（不是整段丢弃）。
  assert.ok(boundedText.length > 30 * 1024, "截断保留前缀而非清空");
  // 注意：boundDisplayText 是对**已序列化 JSON** 按字节切前缀再拼后缀，切口落在字符串
  // 字面量内部 → 截断后的 structuredContent 不保证仍是可解析 JSON（display 是展示投影，
  // UI cuaResultState 解析失败即走降级）。这里只锁真实语义：前缀 + 后缀 + 上限。
  assert.ok(
    boundedStructured.startsWith('{"blob":"'),
    "structuredContent 截断后仍是原 JSON 前缀",
  );
});

test("request_access → permissionStatus 省略（win32 无 darwin meta），schema 仍接受", () => {
  const display = createToolResultDisplay("mcp__computer_use__request_access", requestAccessOutput, {
    officialCua: true,
  });
  assertDisplayParses(display, "request_access");
  const d = display as UnknownRecord;
  assert.equal(d.status, "success");
  assert.equal("permissionStatus" in d, false, "Windows 合成结果不带 darwin meta → 省略");
  assert.equal((JSON.parse(d.structuredContent as string) as UnknownRecord).platform, "windows");

  // 即便塞入非法（非 darwin）status meta，也因 safeParse 失败而省略（result-display.ts:308-327）。
  const spoofed = createToolResultDisplay(
    "mcp__computer_use__request_access",
    {
      ...requestAccessOutput,
      _meta: {
        [CUA_REQUEST_ACCESS_STATUS_META_KEY]: {
          schemaVersion: 1,
          platform: "windows",
          grantOwner: "user",
          accessibility: "granted",
          screenRecording: "granted",
        },
      },
    },
    { officialCua: true },
  );
  assertDisplayParses(spoofed, "spoofed request_access");
  assert.equal("permissionStatus" in (spoofed as UnknownRecord), false, "非 darwin 载荷安全失败 → 省略");
});

test("targetApp _meta → display 投影；officialCua=false 关门", () => {
  const output = {
    isError: false,
    content: [{ type: "text", text: JSON.stringify([{ pid: 7, name: "Notepad" }]) }],
    _meta: { [CUA_TARGET_APP_DISPLAY_META_KEY]: VALID_TARGET_APP },
  };
  const display = createToolResultDisplay("mcp__computer_use__list_apps", output, {
    officialCua: true,
  });
  assertDisplayParses(display, "targetApp 携带");
  const d = display as UnknownRecord;
  assert.deepEqual(d.targetApp, VALID_TARGET_APP, "合法 windows-executable-path 形状原样投影");
  assert.equal("structuredContent" in d, false, "裸数组文本无 structuredContent（runtime list_apps 面）");

  const gated = createToolResultDisplay("mcp__computer_use__list_apps", output, {
    officialCua: false,
  });
  assertDisplayParses(gated, "targetApp 关门");
  assert.equal("targetApp" in (gated as UnknownRecord), false, "非官方 CUA 不投影 targetApp");
});
