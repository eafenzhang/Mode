//! rd_media 纯逻辑（spec: docs/specs/lan-remote-desktop-native-media.md 测试面）。
use mode_cua_ax::rd_media::state::{frame_flags, reduce, EncoderAction, EncoderState};

#[test]
fn gop_reduces_to_idr_every_120_frames() {
    let mut s = EncoderState::new(120);
    let mut idr_count = 0;
    let mut key_seqs = Vec::new();
    for _ in 0..360 {
        let a = reduce(&mut s);
        // 只收集 key 分支：既数总量也钉位置，区间 120..=179 挪位不再能蒙混过关。
        if let EncoderAction::Emit { seq, key: true } = a {
            idr_count += 1;
            key_seqs.push(seq);
        }
    }
    assert_eq!(idr_count, 3, "360 帧 / GOP120 应恰好 3 个关键帧");
    assert_eq!(key_seqs, [0, 120, 240], "关键帧 seq 必须恰为 0/120/240");
}

#[test]
fn gop_of_one_emits_idr_every_frame() {
    // gop.max(1) 许可 gop=1：每帧都必须是 IDR，这是「每 GOP 一个 IDR」的下边界。
    let mut s = EncoderState::new(1);
    for _ in 0..8 {
        let a = reduce(&mut s);
        assert!(a_is_key(&a), "gop=1 时每帧都必须是 Emit{{key:true}}");
    }
}

fn a_is_key(a: &EncoderAction) -> bool {
    matches!(a, EncoderAction::Emit { key: true, .. })
}

#[test]
fn force_idr_after_switch_emits_key_next_frame() {
    let mut s = EncoderState::new(120);
    for _ in 0..10 { let _ = reduce(&mut s); }
    reduce_force_idr(&mut s);
    assert!(matches!(reduce(&mut s), EncoderAction::Emit { key: true, seq: 10, .. }));
}

fn reduce_force_idr(s: &mut EncoderState) {
    s.force_idr();
}

#[test]
fn freeze_halts_emit_and_unfreeze_forces_idr() {
    let mut s = EncoderState::new(120);
    s.set_frozen(true);
    assert!(matches!(reduce(&mut s), EncoderAction::None));
    s.set_frozen(false);
    assert!(matches!(reduce(&mut s), EncoderAction::Emit { key: true, .. }));
}

#[test]
fn frame_flags_key_bit_only_on_idr() {
    assert_eq!(frame_flags(false), 0);
    assert_eq!(frame_flags(true), 1);
}
