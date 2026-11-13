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

/// 采集帧率节拍器（spec: docs/specs/remote-desktop-latency.md §2.1）。
///
/// WHY：采集循环原来是 `next_frame(16)` 的死循环，显示器刷新率就是编码帧率——
/// 80/144Hz 屏会按 80/144fps 编，而编码器按预设 fps 配码率，直接造成码率超发与
/// 链路排队。这里把「允许下一帧的时刻」变成显式状态，worker 只在到期后取帧/编码。
///
/// 语义与边界：
/// - 间隔按 `ceil(1000/fps)` 取整，保证实际速率**不超过**预设（60 → 17ms、30 → 34ms）；
/// - `on_emitted` 从本帧时刻重新排期，不承接欠账（落后时不会连发补帧）；
/// - 改 fps 立即生效：`set_fps` 把到期时刻复位到当前，下一 tick 即可出帧。
pub struct FramePacer {
    fps: u32,
    interval_ms: u64,
    next_at_ms: u64,
}

fn interval_ms_for(fps: u32) -> u64 {
    let fps = fps.max(1) as u64;
    (1000 + fps - 1) / fps
}

impl FramePacer {
    pub fn new(fps: u32, now_ms: u64) -> Self {
        let fps = fps.max(1);
        Self { fps, interval_ms: interval_ms_for(fps), next_at_ms: now_ms }
    }

    pub fn fps(&self) -> u32 { self.fps }
    pub fn interval_ms(&self) -> u64 { self.interval_ms }

    /// 距离允许下一帧还剩多少毫秒；0 = 可以立即出帧。
    pub fn wait_ms(&self, now_ms: u64) -> u64 { self.next_at_ms.saturating_sub(now_ms) }

    /// 改帧率：相同值不改排期；不同值从当前时刻重新开始，避免旧欠账拖慢新档。
    pub fn set_fps(&mut self, fps: u32, now_ms: u64) {
        let fps = fps.max(1);
        if fps == self.fps {
            return;
        }
        self.fps = fps;
        self.interval_ms = interval_ms_for(fps);
        self.next_at_ms = now_ms;
    }

    /// 本 tick 已把一帧交给编码器：按当前 fps 排下一帧。
    pub fn on_emitted(&mut self, now_ms: u64) {
        self.next_at_ms = now_ms.saturating_add(self.interval_ms);
    }
}

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

/// 从 Annex-B 字节流中探测 HEVC 随机访问点 NAL（BLA 16..18 / IDR 19..20 / CRA 21，
/// 即 `nal_unit_type = (首字节 >> 1) & 0x3f` 落在 16..=21；首字节高位为 forbidden_zero_bit，
/// 必须为 0）。
///
/// WHY 分 codec 而不嗅探：HEVC 与 H.264 的首字节编码空间交叠（如 H.264 SEI ref_idc=1
/// 的 0x26 按 HEVC 解读恰是 type19=IDR），混判会把 H.264 关键帧流误标——判定权在
/// 编码器自身已知的 codec（`Encoder::take_output` 按 codec 分支选择本函数或 `contains_idr`）。
pub fn contains_hevc_idr(data: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            let (nal_at, is_start) = if data[i + 2] == 1 {
                (i + 3, true)
            } else if data[i + 2] == 0 && i + 4 <= data.len() && data[i + 3] == 1 {
                (i + 4, true)
            } else {
                (0, false)
            };
            if is_start {
                if nal_at < data.len() {
                    let b = data[nal_at];
                    // forbidden_zero_bit（HEVC 首字节最高位）必须为 0 才按 HEVC 解读。
                    if b & 0x80 == 0 {
                        let nal_type = (b >> 1) & 0x3f;
                        if (16..=21).contains(&nal_type) {
                            return true;
                        }
                    }
                }
                i = nal_at + 1;
                continue;
            }
        }
        i += 1;
    }
    false
}
