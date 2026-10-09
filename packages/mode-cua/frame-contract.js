// 官方 CUA 帧契约（producer 侧唯一实现）。规范源：docs/specs/computer-use-windows-runtime.md
// §帧契约；消费方：core image-normalization / result-content-projection / result-serialization /
// call-runner、adapters tool-result-media-projection、node-repl-host result。
// 本包零运行时依赖（node: 内置），禁止反向依赖 core/services/ui。
import { createHash, randomBytes } from "node:crypto";

export const OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY = "mode.cua/official-frame-integrity-v1";

export const OFFICIAL_CUA_FRAME_MODEL_CONTENT_PROTECTION = "official_cua_frame_v1";

export const OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES = 200 * 1024;

// base64 与原始字节 4:3 的内联预算口径。通用 MCP 预算在 core 侧独立定义
// （MCP_IMAGE_INLINE_RAW_BYTES），这里不能 import——按同值换算，语义各自独立。
const OFFICIAL_CUA_IMAGE_INLINE_RAW_BYTES = Math.floor((OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES * 3) / 4);

// 降级文案：字面复制自 apps/mode-cli/packages/core/src/runtime/helpers/official-cua-media.ts
// 的 OFFICIAL_CUA_RASTER_UNAVAILABLE_TEXT（依赖方向强制 mode-cua 不得 import core，故自持一份）。
// core 侧改动该常量时必须同步这里，测试 frame-contract.test.mjs 有逐字相等断言。
const OFFICIAL_CUA_RASTER_UNAVAILABLE_TEXT =
  "This CUA raster is not visible in this request. " +
  "Do not send a coordinate target; capture a new raster first.";

function isNonEmptyString(value) {
  return typeof value === "string" && value.length > 0;
}

// 整块单行 JSON 解析：只认「trim 后整段就是这个 JSON 对象」，prose 内嵌的字段名
// 不误杀（image-normalization 剥离判据同此）。返回内部 image_ref 对象或 undefined。
function parseImageRefBlock(text) {
  if (!isNonEmptyString(text)) return undefined;
  const trimmed = text.trim();
  if (!trimmed.startsWith("{")) return undefined;
  let parsed;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    return undefined;
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return undefined;
  const ref = parsed.image_ref;
  if (!ref || typeof ref !== "object" || Array.isArray(ref)) return undefined;
  if (!isNonEmptyString(ref.frame_id) || !isNonEmptyString(ref.credential)) return undefined;
  return ref;
}

// 从 start 处的 "{" 起做括号配对（尊重字符串与转义），返回匹配的 "}" 下标或 -1。
function matchJsonBraces(text, start) {
  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let index = start; index < text.length; index += 1) {
    const char = text[index];
    if (inString) {
      if (escaped) escaped = false;
      else if (char === "\\") escaped = true;
      else if (char === '"') inString = false;
      continue;
    }
    if (char === '"') inString = true;
    else if (char === "{") depth += 1;
    else if (char === "}") {
      depth -= 1;
      if (depth === 0) return index;
    }
  }
  return -1;
}

// 内嵌扫描：在任意文本里找可解析 JSON 对象的顶层 image_ref，且该 ref 必须满足
// predicate 才返回（不满足则继续扫，防止「先抛一个缺字段的假 ref、真 ref 藏后面」绕过）。
// 存在安全义务（adapters undeliverableFrameReferenceText 注释钉死）：把 ref JSON
// 包进说明文字不得绕过凭证过滤——安全边界不依赖 payload 形状。
function findEmbeddedImageRef(text, predicate) {
  if (!isNonEmptyString(text)) return undefined;
  // 快速否定只允许在「文本无反斜杠」时生效：没有反斜杠就不存在 JSON 转义键
  // （键首字符用 Unicode 转义写法表示字母 i——JSON.parse 归一化后仍是 image_ref，整块判据照样接受），
  // 此时合法键必以字面 "image_ref" 出现，缺字面即可判否；带反斜杠的文本
  // 一律落全量扫描，否则转义键会只骗过内嵌谓词、让两个判据在同一输入上互相矛盾
  // （安全边界见 adapters tool-result-media-projection.ts 内嵌扫描注释）。
  if (!text.includes("\\") && !text.includes('"image_ref"')) return undefined;
  let from = 0;
  for (;;) {
    const open = text.indexOf("{", from);
    if (open === -1) return undefined;
    from = open + 1;
    const close = matchJsonBraces(text, open);
    if (close === -1) continue;
    let parsed;
    try {
      parsed = JSON.parse(text.slice(open, close + 1));
    } catch {
      continue;
    }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) continue;
    const ref = parsed.image_ref;
    if (ref && typeof ref === "object" && !Array.isArray(ref) && predicate(ref)) return ref;
  }
}

export function isOfficialCuaImageRefText(text) {
  return parseImageRefBlock(text) !== undefined;
}

export function containsOfficialCuaImageRefCredentialText(text) {
  return findEmbeddedImageRef(text, (ref) => isNonEmptyString(ref.credential)) !== undefined;
}

export function containsImageRefAuthority(text) {
  return (
    findEmbeddedImageRef(
      text,
      (ref) => isNonEmptyString(ref.credential) && isNonEmptyString(ref.frame_id),
    ) !== undefined
  );
}

export function parseOfficialCuaImageRef(text) {
  return parseImageRefBlock(text);
}

// 栅格信封身份 = sha256(base64 解码字节) 的小写 hex。同时接受 MCP 形状
// （data 裸 base64 / data URL）与 AI SDK 模型块形状（dataUrl），后者是 call-runner
// 终检 attest 时的真实载荷形态（formatMcpToolResult 会把 data 转成 dataUrl）。
export function readRasterEnvelopeIdentity(input) {
  const payload = readBase64Payload(input);
  if (payload === undefined) return undefined;
  const bytes = Buffer.from(payload, "base64");
  if (bytes.length === 0) return undefined;
  return createHash("sha256").update(bytes).digest("hex");
}

// 相邻对 (i, i+1) 扫描。一栅格规则：恰好一对才返回；0 对或 ≥2 对一律 undefined
// （node-repl host 的 keepLatest 已做「最新者胜」去重，这里锁 fail-closed）。
export function findOfficialCuaFrameContentPair(content) {
  const indexes = findFramePairIndexes(content);
  if (indexes.length !== 1) return undefined;
  const imageIndex = indexes[0];
  return {
    image: content[imageIndex],
    imageRef: content[imageIndex + 1],
    imageIndex,
    imageRefIndex: imageIndex + 1,
  };
}

// 精确栅格闸门，三级语义（authority 路径由调用方保证）：
// 1) ref.raster_sha256 与解码字节真实 digest 不符 → 整帧 fail-closed（移除 image+ref，
//    替换为与 core official-cua-media 同字面的不可见提示）；
// 2) base64 > 200KiB → 经 imageProcessorPort.prepareForModel 压到限内；压不进（端口缺失/
//    异常/仍超限/尺寸被改变）→ 同样降级；压缩成功则字节已变，必须重签 ref 的
//    raster_sha256（否则 call-runner 终检 attest 必抛），frame_id/credential/width/height
//    是帧身份与坐标空间，保持不变；
// 3) 通过 → 原样保留帧与 meta（返回同一对象引用）。
export async function preserveOfficialCuaFrameResult(result, options = {}) {
  const content = result?.content;
  if (!Array.isArray(content)) return result;
  const pairIndexes = findFramePairIndexes(content);
  if (pairIndexes.length === 0) return result;
  if (pairIndexes.length > 1) return degradeOfficialCuaFrameResult(result, pairIndexes);

  const imageIndex = pairIndexes[0];
  const image = content[imageIndex];
  const refBlock = content[imageIndex + 1];
  const ref = parseOfficialCuaImageRef(refBlock.text);
  const actualDigest = readRasterEnvelopeIdentity(image);
  if (
    !ref ||
    !isNonEmptyString(ref.raster_sha256) ||
    actualDigest === undefined ||
    ref.raster_sha256 !== actualDigest
  ) {
    return degradeOfficialCuaFrameResult(result, pairIndexes);
  }

  const payload = readBase64Payload(image);
  if (payload === undefined) return degradeOfficialCuaFrameResult(result, pairIndexes);
  if (Buffer.byteLength(payload, "utf8") <= OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES) return result;

  const port = options.imageProcessorPort;
  const width = ref.width;
  const height = ref.height;
  const mediaType = isNonEmptyString(ref.mimeType)
    ? ref.mimeType
    : isNonEmptyString(image.mimeType)
      ? image.mimeType
      : undefined;
  if (
    !port ||
    typeof port.prepareForModel !== "function" ||
    !Number.isFinite(width) ||
    !Number.isFinite(height) ||
    width <= 0 ||
    height <= 0 ||
    !isNonEmptyString(mediaType)
  ) {
    return degradeOfficialCuaFrameResult(result, pairIndexes);
  }

  const raw = Buffer.from(payload, "base64");
  if (raw.length === 0) return degradeOfficialCuaFrameResult(result, pairIndexes);
  let prepared;
  try {
    // 端口形状对齐 core 调用点（image-normalization.ts tryCompressHostNodeReplImage）：
    // prepareForModel(ImagePrepareForModelRequest, { signal })。maxDimension 取原图长边，
    // 借 resizeToFit 的「只缩不放」语义阻止端口改变坐标空间尺寸。
    prepared = await port.prepareForModel(
      {
        data: raw,
        mediaType,
        maxDimension: Math.max(width, height),
        maxBase64Bytes: OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES,
        maxRawBytes: OFFICIAL_CUA_IMAGE_INLINE_RAW_BYTES,
      },
      { signal: options.signal },
    );
  } catch {
    // 端口异常（含取消）按「压不进」降级：模型拿到真话比拿到崩溃安全，取消链路由上游收口。
    return degradeOfficialCuaFrameResult(result, pairIndexes);
  }
  if (
    !prepared ||
    !(prepared.data instanceof Uint8Array) ||
    prepared.data.length === 0 ||
    !isNonEmptyString(prepared.mediaType) ||
    !prepared.mediaType.startsWith("image/")
  ) {
    return degradeOfficialCuaFrameResult(result, pairIndexes);
  }
  // 尺寸被端口改变 → ref 的 width/height 不再描述模型所见栅格，坐标契约不可静默漂移。
  if (
    (prepared.width !== undefined && prepared.width !== width) ||
    (prepared.height !== undefined && prepared.height !== height)
  ) {
    return degradeOfficialCuaFrameResult(result, pairIndexes);
  }
  const compressedPayload = Buffer.from(prepared.data).toString("base64");
  if (Buffer.byteLength(compressedPayload, "utf8") > OFFICIAL_CUA_IMAGE_INLINE_BASE64_BYTES) {
    return degradeOfficialCuaFrameResult(result, pairIndexes);
  }
  const compressedDigest = createHash("sha256").update(prepared.data).digest("hex");

  const nextContent = [...content];
  nextContent[imageIndex] = { ...image, data: compressedPayload, mimeType: prepared.mediaType };
  nextContent[imageIndex + 1] = {
    ...refBlock,
    text: JSON.stringify({
      image_ref: {
        frame_id: ref.frame_id,
        credential: ref.credential,
        raster_sha256: compressedDigest,
        width,
        height,
        mimeType: prepared.mediaType,
      },
    }),
  };
  return { ...result, content: nextContent };
}

// 终检：image@0 + ref@1 + ref.raster_sha256 与真实栅格 digest 相符 → 签发真值包。
// 否则 undefined——call-runner 据此在「有 image 却无真值 attestation」时抛
// "final model-content attestation" 失败（本函数保护的消费方）。
export function attestOfficialCuaFrameContent(content, expectedKind) {
  const kind =
    expectedKind === undefined ? OFFICIAL_CUA_FRAME_MODEL_CONTENT_PROTECTION : expectedKind;
  // 当前契约只存在一种帧保护 kind；未知 kind 不签发凭证。
  if (kind !== OFFICIAL_CUA_FRAME_MODEL_CONTENT_PROTECTION) return undefined;
  if (!Array.isArray(content)) return undefined;
  const pair = findOfficialCuaFrameContentPair(content);
  if (!pair || pair.imageIndex !== 0 || pair.imageRefIndex !== 1) return undefined;
  const ref = parseOfficialCuaImageRef(pair.imageRef.text);
  if (!ref || !isNonEmptyString(ref.raster_sha256)) return undefined;
  const actualDigest = readRasterEnvelopeIdentity(pair.image);
  if (actualDigest === undefined || actualDigest !== ref.raster_sha256) return undefined;
  return { kind };
}

export function buildOfficialCuaImageRefText(options) {
  const { frameId, rasterSha256, width, height, mimeType } = options ?? {};
  // 单行 JSON（SDK frameIdOf 读 parsed.image_ref.frame_id）；credential 每帧随机。
  return JSON.stringify({
    image_ref: {
      frame_id: frameId,
      credential: randomBytes(16).toString("hex"),
      raster_sha256: rasterSha256,
      width,
      height,
      mimeType,
    },
  });
}

export function buildOfficialCuaFrameIntegrityMeta(options) {
  const { frameId, rasterSha256 } = options ?? {};
  // 放 _meta[OFFICIAL_CUA_FRAME_INTEGRITY_META_KEY]；消费方（hasOfficialCuaFrameAuthority）
  // 只查存在性，真值即可。
  return { v: 1, frame_id: frameId, raster_sha256: rasterSha256 };
}

function findFramePairIndexes(content) {
  if (!Array.isArray(content)) return [];
  const indexes = [];
  for (let index = 0; index + 1 < content.length; index += 1) {
    const block = content[index];
    if (!block || typeof block !== "object" || block.type !== "image") continue;
    const next = content[index + 1];
    if (!next || typeof next !== "object" || next.type !== "text") continue;
    if (isOfficialCuaImageRefText(next.text)) indexes.push(index);
  }
  return indexes;
}

// 整帧降级：移除全部帧对（image+ref），在首对位置插入不可见提示文本；
// 其余块与 _meta/structuredContent/isError 原样保留。
function degradeOfficialCuaFrameResult(result, pairIndexes) {
  const removed = new Set(pairIndexes.flatMap((index) => [index, index + 1]));
  const noticeIndex = pairIndexes[0];
  const content = [];
  for (const [index, block] of result.content.entries()) {
    if (index === noticeIndex) {
      content.push({ type: "text", text: OFFICIAL_CUA_RASTER_UNAVAILABLE_TEXT });
    }
    if (removed.has(index)) continue;
    content.push(block);
  }
  return { ...result, content };
}

// 取 image 块的裸 base64 载荷：MCP data（裸 base64 或 data URL）与 AI SDK dataUrl
// 两种形状都接受；无法取出 → undefined（fail-closed）。
function readBase64Payload(input) {
  if (!input || typeof input !== "object" || input.type !== "image") return undefined;
  const raw =
    typeof input.data === "string" ? input.data : typeof input.dataUrl === "string" ? input.dataUrl : undefined;
  if (raw === undefined) return undefined;
  if (raw.startsWith("data:")) {
    const comma = raw.indexOf(",");
    if (comma === -1) return undefined;
    const payload = raw.slice(comma + 1);
    return payload.length > 0 ? payload : undefined;
  }
  return raw.length > 0 ? raw : undefined;
}
