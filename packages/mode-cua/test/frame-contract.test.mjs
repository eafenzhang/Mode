// 帧契约测试（Plan B Task 1）。基准用例来自任务 brief；两处修订有据：
// 1) 压缩端口形状：brief 草稿写 `{ compress }`，真实接口是 image-normalization.ts:41-44
//    传入的 ImageProcessorPort（contracts image-processor.port.ts）：
//    prepareForModel(request, { signal }) → { data: Uint8Array, mediaType, ... }。
// 2) 降级文案断言升级为与 core OFFICIAL_CUA_RASTER_UNAVAILABLE_TEXT 逐字相等
//    （binding contract：mode-cua 禁止 import core，字面必须自持且一致）。
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

const png = (n) => Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47]), Buffer.alloc(n, 7)]);
const b64 = (buf) => buf.toString("base64");
const digest = (buf) => createHash("sha256").update(buf).digest("hex");

// 与 apps/mode-cli/packages/core/src/runtime/helpers/official-cua-media.ts 的
// OFFICIAL_CUA_RASTER_UNAVAILABLE_TEXT 同源字面（逐字断言降级文案）。
const RASTER_UNAVAILABLE_TEXT =
  "This CUA raster is not visible in this request. " +
  "Do not send a coordinate target; capture a new raster first.";

function pair(buf) {
  return [
    { type: "image", data: b64(buf), mimeType: "image/png" },
    { type: "text", text: buildOfficialCuaImageRefText({
      frameId: randomBytes(8).toString("hex"), rasterSha256: digest(buf),
      width: 4, height: 4, mimeType: "image/png" }) },
  ];
}

test("ref 文本整块识别 + credential/解析", () => {
  const [, ref] = pair(png(64));
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
  const [img2, ref2] = pair(png(70));
  assert.equal(findOfficialCuaFrameContentPair([img, ref, img2, ref2]), undefined);
});

test("attest：对完好对返回真值，缺 meta/digest 不符 → falsy", () => {
  const buf = png(64); const [img, ref] = pair(buf);
  assert.ok(attestOfficialCuaFrameContent([img, ref], "official_cua_frame_v1"));
  assert.equal(attestOfficialCuaFrameContent([{ type: "text", text: "x" }], "official_cua_frame_v1"), undefined);
  // 生产形状：call-runner 终检的是 formatMcpToolResult 产出的 AI SDK 块（dataUrl 载荷），
  // 只认 MCP data 字段会在这里假失败并让 call-runner 误抛 attestation 错。
  const dataUrlImg = { type: "image", dataUrl: `data:image/png;base64,${b64(buf)}`, mediaType: "image/png" };
  assert.ok(attestOfficialCuaFrameContent([dataUrlImg, ref], "official_cua_frame_v1"));
  // 未知 kind 不签发凭证（fail-closed）。
  assert.equal(attestOfficialCuaFrameContent([img, ref], "other_kind_v9"), undefined);
});

test("preserve：digest 不符 → 整帧 fail-closed 替换", async () => {
  const buf = png(64); const [img, ref] = pair(buf);
  const tampered = { ...img, data: b64(png(65)) };
  const out = await preserveOfficialCuaFrameResult(
    { content: [tampered, ref], _meta: { "mode.cua/official-frame-integrity-v1": { v: 1 } } },
    { imageProcessorPort: undefined, signal: undefined });
  assert.equal(out.content.some((b) => b.type === "image"), false);
  assert.match(out.content[0].text, /not visible|不可见/u);
  // 降级文案与 core 逐字一致（binding：字面复制 + 同步义务）。
  assert.equal(out.content[0].text, RASTER_UNAVAILABLE_TEXT);
  // 移除 image+ref，只留提示块。
  assert.equal(out.content.length, 1);
});

test("preserve：>200KiB 走压缩端口，压不进 → 降级不可见提示", async () => {
  const big = png(300 * 1024);
  const meta = { "mode.cua/official-frame-integrity-v1": { v: 1, frame_id: "f", raster_sha256: digest(big) } };
  const [img, ref] = pair(big);
  const compressed = png(100 * 1024);
  const seen = {};
  const ok = await preserveOfficialCuaFrameResult({ content: [img, ref], _meta: meta },
    { imageProcessorPort: { prepareForModel: async (request, options) => {
      seen.request = request; seen.options = options;
      return { data: compressed, mediaType: "image/jpeg" };
    } }, signal: undefined });
  assert.equal(ok.content[0].type, "image");
  assert.equal(ok.content[0].mimeType, "image/jpeg");
  // 端口请求形状与 core 调用点一致（ImagePrepareForModelRequest）。
  assert.equal(seen.request.mediaType, "image/png");
  assert.equal(seen.request.maxBase64Bytes, 200 * 1024);
  assert.equal(seen.request.maxRawBytes, 150 * 1024);
  assert.equal(seen.request.maxDimension, 4);
  assert.ok(seen.request.data instanceof Uint8Array);
  assert.ok(seen.options.signal === undefined);
  // 压缩后必须重签 ref：raster_sha256 = 新字节 digest，否则 call-runner 终检必抛；
  // frame_id/credential/width/height 是帧身份与坐标空间，保持不变。
  const rebuilt = parseOfficialCuaImageRef(ok.content[1].text);
  const original = parseOfficialCuaImageRef(ref.text);
  assert.equal(rebuilt.raster_sha256, digest(compressed));
  assert.equal(rebuilt.frame_id, original.frame_id);
  assert.equal(rebuilt.credential, original.credential);
  assert.equal(rebuilt.mimeType, "image/jpeg");
  const fail = await preserveOfficialCuaFrameResult({ content: [img, ref], _meta: meta },
    { imageProcessorPort: { prepareForModel: async () => undefined }, signal: undefined });
  assert.equal(fail.content.some((b) => b.type === "image"), false);
  assert.equal(fail.content[0].text, RASTER_UNAVAILABLE_TEXT);
});

test("preserve：帧内尺寸被端口改变 → 拒绝（坐标契约不可静默漂移）", async () => {
  const big = png(300 * 1024);
  const [img, ref] = pair(big);
  const out = await preserveOfficialCuaFrameResult({ content: [img, ref] },
    { imageProcessorPort: { prepareForModel: async () => ({ data: png(100 * 1024), mediaType: "image/jpeg", width: 2, height: 2 }) }, signal: undefined });
  assert.equal(out.content.some((b) => b.type === "image"), false);
  assert.equal(out.content[0].text, RASTER_UNAVAILABLE_TEXT);
});

test("preserve：≤200KiB 且 digest 相符 → 原样返回，不触碰压缩端口", async () => {
  const buf = png(64); const [img, ref] = pair(buf);
  const input = { content: [img, ref], _meta: { "mode.cua/official-frame-integrity-v1": { v: 1, frame_id: "f", raster_sha256: digest(buf) } } };
  const out = await preserveOfficialCuaFrameResult(input, {
    imageProcessorPort: { prepareForModel: async () => { throw new Error("must not compress"); } },
    signal: undefined,
  });
  assert.equal(out, input);
  // 无帧对（纯文本观察/错误结果）同样原样通过。
  const textOnly = { content: [{ type: "text", text: "app: notepad" }], isError: false };
  assert.equal(await preserveOfficialCuaFrameResult(textOnly, {}), textOnly);
});

test("构建器：单行 ref JSON + 每帧随机 credential + meta 形状", () => {
  const frameId = randomBytes(8).toString("hex");
  const rasterSha256 = digest(png(16));
  const text = buildOfficialCuaImageRefText({
    frameId, rasterSha256, width: 1920, height: 1080, mimeType: "image/png" });
  assert.equal(text.includes("\n"), false);                 // 单行 JSON 块
  const parsed = parseOfficialCuaImageRef(text);
  assert.equal(parsed.frame_id, frameId);
  assert.equal(parsed.raster_sha256, rasterSha256);
  assert.equal(parsed.width, 1920);
  assert.equal(parsed.height, 1080);
  assert.match(parsed.credential, /^[0-9a-f]{32}$/u);       // randomBytes(16).toString("hex")
  const again = buildOfficialCuaImageRefText({
    frameId, rasterSha256, width: 1920, height: 1080, mimeType: "image/png" });
  assert.notEqual(parsed.credential, parseOfficialCuaImageRef(again).credential); // 每帧随机
  assert.deepEqual(buildOfficialCuaFrameIntegrityMeta({ frameId, rasterSha256 }),
    { v: 1, frame_id: frameId, raster_sha256: rasterSha256 });
});

test("凭证/authority 谓词是内嵌扫描：包进说明文字不得绕过 fail-closed", () => {
  const [, ref] = pair(png(64));
  // adapters undeliverableFrameReferenceText / result-serialization hook 过滤的
  // 共享 detector 契约：ref JSON 被包进 prose 仍必须被识别（安全边界不依赖 payload 形状）。
  assert.equal(containsOfficialCuaImageRefCredentialText("explanation… " + ref.text + " …end"), true);
  assert.equal(isOfficialCuaImageRefText("explanation… " + ref.text), false); // 整块判定保持严格
  assert.equal(containsOfficialCuaImageRefCredentialText('the "image_ref" field matters'), false);
  assert.equal(containsImageRefAuthority(ref.text), true);
  assert.equal(containsImageRefAuthority('{"image_ref":{"credential":"abcdef0123456789"}}'), false); // 缺 frame_id
});

test("内嵌扫描：转义键归一化后不得旁路，无键纯文本仍走快路", () => {
  // JSON 键首字符写成 Unicode 转义（字母 i 的转义写法）：JSON.parse 归一化为 image_ref，整块判据接受；
  // 内嵌谓词必须同受约束——否则 tool-result-media-projection.ts:212 的
  // 安全边界（包进说明文字不可绕过）会被转义键打穿，两个谓词在同一输入上互相矛盾。
  const escapedBlock =
    '{"\\u0069mage_ref":{"frame_id":"f-esc","credential":"abcdef0123456789",' +
    '"raster_sha256":"aa","width":4,"height":4,"mimeType":"image/png"}}';
  assert.equal(isOfficialCuaImageRefText(escapedBlock), true); // 整块：JSON.parse 归一化
  assert.equal(containsOfficialCuaImageRefCredentialText(escapedBlock), true);
  assert.equal(containsImageRefAuthority(escapedBlock), true);
  const wrapped = "explanation… " + escapedBlock + " …end";
  assert.equal(containsOfficialCuaImageRefCredentialText(wrapped), true); // 内嵌 + 转义键
  assert.equal(containsImageRefAuthority(wrapped), true);
  // 快路完整性：无键且无反斜杠的纯文本直接判否；
  // 含反斜杠但无 ref 的文本落全量扫描后同样判否（扫描不产生假阳性）。
  assert.equal(containsOfficialCuaImageRefCredentialText('app: notepad pid=1 "element"'), false);
  assert.equal(containsImageRefAuthority("path C:\\temp {\"a\":1} done"), false);
});
