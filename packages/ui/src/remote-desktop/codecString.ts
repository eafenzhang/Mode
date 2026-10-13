/**
 * WebCodecs codec 串派生与解码能力预检（**码流自描述**，spec: native-media
 * 「HEVC 选型」2026-10-13；从 nativeMediaSession 拆出——max-lines 约束）：
 * 被控实际用什么 codec 编码，这里就解出什么串，偏好与实际不一致安全。
 */
/**
 * Annex-B 关键帧 → WebCodecs codec 串（**码流自描述**，spec: native-media「HEVC 选型」
 * 2026-10-13）——被控实际用什么 codec 编码，这里就解出什么串，偏好与实际不一致安全：
 * - H.264：首个 SPS（NAL type 7）→ "avc1.<profile><constraint><level>"（hex，小写）；
 * - HEVC：首个 VPS/SPS（NAL type 32/33，首字节 0x40..0x43）判族 → 找 SPS(type 33)
 *   解析 profile_tier_level → `hev1.<profile_idc>.<compat hex 大写>.<tier><level_idc>`
 *   [+ `.<约束字节 hex>`]（对齐 Chromium 生成式；约束字节 = progressive/interlaced/
 *   non_packed/frame_only 四位放高 nibble，全 0 省略段）。
 * 族判定只认 type 32/33：关键帧里 H.264 只会含 5/6/7/8/9 型（0x65/0x06/0x67/0x68/0x09），
 * 按 HEVC 解读的 type 均不落 32/33；而 0x45（HEVC PPS 可能值）恰是 H.264 IDR ref2——
 * 故不用 34 判族，避免关键帧 IDR 误判。
 * 关键帧里的参数集是分辨率/编码参数变化的唯一信号（spec：新参数集触发 decoder 重配）。
 * 截断/缺 SPS → null（调用方沿用「keyframe without SPS」分支）。
 */
export function codecStringFromAnnexB(bytes: Uint8Array): string | null {
  for (let i = 0; i + 5 < bytes.length; i++) {
    const is4 = bytes[i] === 0 && bytes[i + 1] === 0 && bytes[i + 2] === 0 && bytes[i + 3] === 1;
    const is3 = !is4 && bytes[i] === 0 && bytes[i + 1] === 0 && bytes[i + 2] === 1;
    if (!is4 && !is3) continue;
    const headerIdx = i + (is4 ? 4 : 3);
    if (headerIdx >= bytes.length) continue;
    const b0 = bytes[headerIdx]!;
    const hevcType = (b0 >>> 1) & 0x3f;
    if ((b0 & 0x80) === 0 && (hevcType === 32 || hevcType === 33)) {
      return hevcCodecString(bytes, headerIdx);
    }
    if ((b0 & 0x1f) !== 7 || headerIdx + 3 >= bytes.length) continue;
    const hex = (n: number) => n.toString(16).padStart(2, "0");
    return `avc1.${hex(bytes[headerIdx + 1]!)}${hex(bytes[headerIdx + 2]!)}${hex(bytes[headerIdx + 3]!)}`;
  }
  return null;
}

/** 从判族 NAL（VPS/SPS）起解析 hev1 串：其本身是 SPS 就地解析，否则向后扫 SPS(type 33)。
 *  HEVC NAL 头是 **2 字节**（F+type(6) 跨 layer_id(6) + tid(3)，2026-10-13 实机码流
 *  证：按 1 字节解析会把第二头字节吃进 SPS 载荷，整体位移 → 派生串全错）。 */
function hevcCodecString(bytes: Uint8Array, firstNalHeaderIdx: number): string | null {
  const parseAt = (headerIdx: number): string | null => {
    if (headerIdx + 1 >= bytes.length || ((bytes[headerIdx]! >>> 1) & 0x3f) !== 33) {
      return null;
    }
    return hevcCodecStringFromSps(bytes.subarray(headerIdx + 2));
  };
  const direct = parseAt(firstNalHeaderIdx);
  if (direct !== null) {
    return direct;
  }
  for (let i = firstNalHeaderIdx + 1; i + 5 < bytes.length; i++) {
    const is4 = bytes[i] === 0 && bytes[i + 1] === 0 && bytes[i + 2] === 0 && bytes[i + 3] === 1;
    const is3 = !is4 && bytes[i] === 0 && bytes[i + 1] === 0 && bytes[i + 2] === 1;
    if (!is4 && !is3) continue;
    const headerIdx = i + (is4 ? 4 : 3);
    const found = parseAt(headerIdx);
    if (found !== null) {
      return found;
    }
  }
  return null;
}

/** SPS RBSP（去 emulation prevention 00 00 03）→ 解析 profile_tier_level 头段。 */
function hevcCodecStringFromSps(payload: Uint8Array): string | null {
  const rbsp: number[] = [];
  for (let i = 0; i < payload.length; i++) {
    if (i + 2 < payload.length && payload[i] === 0 && payload[i + 1] === 0 && payload[i + 2] === 3) {
      rbsp.push(0, 0);
      i += 2;
      continue;
    }
    rbsp.push(payload[i]!);
  }
  let bit = 0;
  const get = (n: number): number => {
    let v = 0;
    for (let k = 0; k < n; k++) {
      const byteIdx = bit >> 3;
      if (byteIdx >= rbsp.length) throw new Error("SPS 截断");
      v = v * 2 + ((rbsp[byteIdx]! >> (7 - (bit & 7))) & 1);
      bit += 1;
    }
    return v;
  };
  try {
    get(4); // sps_video_parameter_set_id
    get(3); // sps_max_sub_layers_minus1
    get(1); // sps_temporal_id_nesting_flag
    get(2); // general_profile_space
    const tierFlag = get(1);
    const profileIdc = get(5);
    const compat = get(32);
    const progressive = get(1);
    const interlaced = get(1);
    const nonPacked = get(1);
    const frameOnly = get(1);
    get(44); // 保留位（inbld/_reserved——MF 输出恒 0，不进 codec 串）
    const levelIdc = get(8);
    const constraintByte =
      (progressive ? 0x80 : 0) | (interlaced ? 0x40 : 0) | (nonPacked ? 0x20 : 0) | (frameOnly ? 0x10 : 0);
    const tier = tierFlag === 1 ? "H" : "L";
    let codec = `hev1.${profileIdc}.${compat.toString(16).toUpperCase()}.${tier}${levelIdc}`;
    if (constraintByte !== 0) {
      codec += `.${constraintByte.toString(16).toUpperCase()}`;
    }
    return codec;
  } catch {
    return null;
  }
}

/**
 * 本机 HEVC 解码能力预检（会话发起前一次性）：Chromium 按平台硬解/软解能力面回答，
 * unsupported → rdStartSession 偏好 h264（被控无硬件编码时同样回退——双向独立降级）。
 * 用代表性串（Main / 兼容位 6 / L4.0 / progressive+frame_only）探测即可：
 * 浏览器对 hev1 的支持按能力面判定，不逐参数集。
 */
export async function probeHevcDecodeSupport(): Promise<boolean> {
  try {
    const result = await VideoDecoder.isConfigSupported({
      codec: "hev1.1.6.L120.90",
      codedWidth: 1280,
      codedHeight: 720,
    });
    return result.supported === true;
  } catch {
    return false;
  }
}
