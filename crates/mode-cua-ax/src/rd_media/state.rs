//! 采集/编码循环的纯状态机（不碰 Win32；单测全在此层）。
pub struct EncoderState {
    seq: u32,
    gop: u32,
    frames_since_idr: u32,
    force_idr: bool,
    frozen: bool,
}

pub enum EncoderAction {
    None,
    Emit { seq: u32, key: bool },
}

impl EncoderState {
    pub fn new(gop: u32) -> Self {
        Self { seq: 0, gop: gop.max(1), frames_since_idr: 0, force_idr: false, frozen: false }
    }
    pub fn force_idr(&mut self) { self.force_idr = true; }
    pub fn set_frozen(&mut self, frozen: bool) {
        if self.frozen && !frozen { self.force_idr = true; } // 解冻续流必须带 IDR
        self.frozen = frozen;
    }
    pub fn is_frozen(&self) -> bool { self.frozen }
}

pub fn reduce(state: &mut EncoderState) -> EncoderAction {
    if state.frozen {
        return EncoderAction::None;
    }
    let key = state.force_idr || state.frames_since_idr == 0;
    let seq = state.seq;
    state.seq = state.seq.wrapping_add(1);
    state.frames_since_idr += 1;
    if key {
        state.force_idr = false;
        // 1 % gop：gop≥2 时等于 1（与原语义一致）；gop=1 时归 0，下一帧仍命中
        // 「首帧必 IDR」条件——否则 gop=1 会退化成每 2 帧一个 IDR，违反「每 GOP 一个 IDR」。
        // gop 由 new(gop.max(1)) 保证 ≥1，此处取模不会除零。
        state.frames_since_idr = 1 % state.gop;
    } else if state.frames_since_idr >= state.gop {
        state.frames_since_idr = 0;
    }
    EncoderAction::Emit { seq, key }
}

pub const FRAME_FLAG_KEY: u8 = 1;
pub fn frame_flags(key: bool) -> u8 { if key { FRAME_FLAG_KEY } else { 0 } }

/// 从 Annex-B 字节流中探测 IDR NAL（nal_unit_type == 5）。
///
/// WHY：编码器侧的 keyframe 标志（`MFT_ENCODER_OUTPUT_DATA_PARAM_KEYFRAME`）在
/// MF 输出面上不可取（输出 sample 上读不到该属性），而 wire 层的 flags bit0 必须与
/// 码流真实可解码性一致——错标关键帧会让对端解码器把 P 帧当 IDR 起点，错漏关键帧
/// 会让解码器错过重同步点。因此以码流本身为唯一权威：起始码 `00 00 01` /
/// `00 00 00 01` 后首字节的低 5 位（nal_unit_type）== 5 即 IDR。
/// （H.264 NAL header：forbidden(1) + nal_ref_idc(2) + nal_unit_type(5)，
///  IDR 首字节常见值 0x65 = ref_idc 3 + type 5，故按低 5 位判断而不是比对 0x65，
///  以兼容 ref_idc 取值不同的实现。）
pub fn contains_idr(data: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 3 < data.len() {
        // 4 字节起始码 00 00 00 01 或 3 字节起始码 00 00 01
        if data[i] == 0 && data[i + 1] == 0 {
            let (nal_at, is_start) = if data[i + 2] == 1 {
                (i + 3, true)
            } else if data[i + 2] == 0 && i + 4 <= data.len() && data[i + 3] == 1 {
                (i + 4, true)
            } else {
                (0, false)
            };
            if is_start {
                if nal_at < data.len() && (data[nal_at] & 0x1f) == 5 {
                    return true;
                }
                i = nal_at + 1; // 跳过该起始码，继续扫描后续 NAL
                continue;
            }
        }
        i += 1;
    }
    false
}
