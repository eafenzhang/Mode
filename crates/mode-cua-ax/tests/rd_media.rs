//! rd_media 纯逻辑（spec: docs/specs/lan-remote-desktop-native-media.md 测试面）。
use mode_cua_ax::rd_media::state::{frame_flags, reduce, EncoderAction, EncoderState};

#[test]
fn gop_reduces_to_idr_every_120_frames() {
    let mut s = EncoderState::new(120);
    let mut idr_count = 0;
    for _ in 0..360 {
        let a = reduce(&mut s);
        // 原计划的双层 if 触发 clippy::collapsible_if（-D warnings 强制），按其建议合并，语义不变。
        if matches!(a, EncoderAction::Emit { .. }) && a_is_key(&a) {
            idr_count += 1;
        }
    }
    assert_eq!(idr_count, 3, "360 帧 / GOP120 应恰好 3 个关键帧");
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
    let _ = s;
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
