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
