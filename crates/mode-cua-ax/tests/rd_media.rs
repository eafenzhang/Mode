//! rd_media 纯逻辑（spec: docs/specs/lan-remote-desktop-native-media.md 测试面）。
use mode_cua_ax::rd_media::state::{contains_idr, frame_flags, reduce, EncoderAction, EncoderState};

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

#[test]
fn stop_without_start_is_idempotent() {
  // 真实 DUPL 在无桌面会话的 CI 可能不可用：stop 空跑必须 Ok。
  mode_cua_ax::rd_media::stop_for_tests();
}

#[test]
fn hardware_h264_encoder_builds_with_async_unlock() {
  // 异步 MFT 解锁回归（2026-10-13，spec: lan-remote-desktop-native-media.md「编码」）：
  // 硬件 H.264 MFT（NVENC / AMF / QSV）普遍是异步编码器，非 MF 管线直接使用时
  // 首次 SetInputType 报 MF_E_TRANSFORM_ASYNC_LOCKED (0xC00D6D77)；漏解锁的现象是
  // 远程桌面会话连上但画面永远不出。激活后未置 MF_TRANSFORM_ASYNC_UNLOCK 时本用例变红。
  let Some(device) = mode_cua_ax::rd_media::capture::create_device_for_tests() else {
    // 无 D3D 设备或设备无 VP 能力（基础显示适配器/无桌面会话的 CI）：环境能力缺失，跳过。
    return;
  };
  match mode_cua_ax::rd_media::encode::Encoder::new(&device, 1920, 1080, 60, 8_000_000) {
    Ok(_encoder) => {}
    // 环境根本没有硬件 H.264 编码器（CI 无 GPU 自适应）：能力面缺失，跳过。
    Err(error) if error.message.contains("无可用硬件 H.264 编码器") => {}
    // 枚举调用本身在受限环境失败：同属能力缺失，跳过。
    Err(error) if error.message.contains("MFTEnumEx(硬件 H.264) 失败") => {}
    Err(error) => panic!("硬件 H.264 编码器应可配置（异步 MFT 必须先解锁）: {error}"),
  }
}

#[test]
fn contains_idr_detects_annexb_idr_nal() {
    // 4 字节起始码 + IDR NAL(0x65)：标准 Annex-B IDR 帧头。
    assert!(contains_idr(&[0, 0, 0, 1, 0x65, 0x88]));
    // 3 字节起始码 + IDR NAL。
    assert!(contains_idr(&[0, 0, 1, 0x65, 0x88]));
    // 非 IDR（0x41 = 非关键 P slice；0x67 = SPS）不得误报。
    assert!(!contains_idr(&[0, 0, 0, 1, 0x41, 0x88]));
    assert!(!contains_idr(&[0, 0, 0, 1, 0x67, 0x42]));
    // 空/过短输入不 panic。
    assert!(!contains_idr(&[]));
    assert!(!contains_idr(&[0, 0, 0, 1]));
}

#[test]
fn to_annex_b_passes_start_code_and_converts_avcc() {
    use mode_cua_ax::rd_media::encode::to_annex_b;
    // 已是 Annex-B：字节原样（零拷贝借用）。
    let annexb = [0, 0, 0, 1, 0x65, 0x88, 0x99];
    assert_eq!(&*to_annex_b(&annexb), &annexb);
    // AVCC（4 字节大端长度前缀序列）→ 起始码分隔。
    let avcc = [
        0, 0, 0, 3, 0x65, 0x88, 0x99, // NAL1: IDR, len=3
        0, 0, 0, 2, 0x41, 0x00, // NAL2: 非 IDR, len=2
    ];
    assert_eq!(
        &*to_annex_b(&avcc),
        &[0, 0, 0, 1, 0x65, 0x88, 0x99, 0, 0, 0, 1, 0x41, 0x00]
    );
    // 非 Annex-B 也非 AVCC（长度越界）：原样返回，不猜着重排。
    let garbage = [1u8, 2, 3, 4, 5];
    assert_eq!(&*to_annex_b(&garbage), &garbage);
    // 短缓冲不 panic。
    assert_eq!(&*to_annex_b(&[0, 1]), &[0, 1]);
}

#[test]
fn compute_out_size_caps_width_keeps_aspect_even() {
    use mode_cua_ax::rd_media::napi::compute_out_size;
    // 2560×1440 被 maxWidth 钳到 1920×1080（比例保持）。
    assert_eq!(compute_out_size(2560, 1440, 1920), (1920, 1080));
    // 已在上限内：原样（含偶数约束）。
    assert_eq!(compute_out_size(1920, 1080, 1920), (1920, 1080));
    // 不放大：小矩形保持原尺寸。
    assert_eq!(compute_out_size(100, 50, 1920), (100, 50));
    // 4K 同 16:9 → 1920×1080。
    assert_eq!(compute_out_size(3840, 2160, 1920), (1920, 1080));
}

#[test]
fn render_cursor_shape_expands_monochrome_masks() {
    use mode_cua_ax::rd_media::capture::render_cursor_shape;
    // 单色形状：width=8, height=16（真实光标 8×8，AND 半 + 颜色半），pitch=1。
    // AND 行 0（data[0]）= 0xFF → 该行全透明；AND 行 1（data[1]）= 0x00 →
    // 该行可见，颜色位全 0 → 不透明黑。
    let data = [
        0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // AND 半（8 行 × pitch 1）
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 颜色半
    ];
    let r = render_cursor_shape(1, 8, 16, 1, &data).expect("单色形状应可渲染");
    assert_eq!((r.w, r.h), (8, 8));
    assert!(r.visible[0..8].iter().all(|v| *v == 0), "AND=1 行必须全透明");
    assert_eq!(&r.visible[8..16], &[1u8; 8][..], "AND=0 行必须可见");
    // 可见像素为不透明黑（BGRA）。
    assert_eq!(&r.bgra[32..36], &[0, 0, 0, 255]);

    // 缓冲不足 → internal 错误（截断输入不 panic）。
    assert!(render_cursor_shape(1, 8, 16, 1, &[0u8; 8]).is_err());
    // 未知形状类型 → 空栅格（不打补丁、不杀流）。
    let unk = render_cursor_shape(99, 8, 8, 4, &[0u8; 32]).expect("未知类型不报错");
    assert_eq!((unk.w, unk.h), (0, 0));
}

#[test]
fn render_cursor_shape_preserves_color_alpha_and_masked_layout() {
    use mode_cua_ax::rd_media::capture::render_cursor_shape;
    // COLOR（官方 32bpp ARGB，alpha 即形状——2026-10-13 修正：此前强推 alpha=255 +
    // 全可见，会把带透明通道的光标画成不透明色块）：
    // 2×1，pitch=8：p0 alpha=128 → 可见且保留真实 alpha；p1 alpha=0 → 不可见（露背景）。
    let color = [
        10u8, 20, 30, 128, // p0：真 alpha（边缘半透明）
        0, 0, 0, 0, // p1：全透明
    ];
    let r = render_cursor_shape(2, 2, 1, 8, &color).expect("颜色形状应可渲染");
    assert_eq!((r.w, r.h), (2, 1));
    assert_eq!(&r.bgra[0..4], &[10, 20, 30, 128], "真实 alpha 必须保留");
    assert_eq!(&r.visible, &[1u8, 0], "visible = alpha>0（全透明像素 = 背景）");
    assert!(r.xor.iter().all(|v| *v == 0), "COLOR 无 XOR 语义");

    // MASKED_COLOR（官方：32bpp ARGB、alpha 位即掩码 0=替换 / 0xFF=XOR、**无尾随
    // 1bpp 掩码**——2026-10-13 修正：旧实现多要一段掩码长度，真实数据必判缓冲不足
    // → 该形状整体跳过 = 光标不可见）。
    let masked = [
        1u8, 2, 3, 0x00, // 替换像素
        4, 5, 6, 0xFF, // XOR 像素
        7, 8, 9, 0x7F, // 非法中间值（官方只允许 0/0xFF）→ 宽容按替换
        0, 0, 0, 0, // 行内填充到 pitch=16
    ];
    let r = render_cursor_shape(4, 4, 1, 16, &masked).expect("掩码颜色按新布局应可渲染");
    assert_eq!((r.w, r.h), (4, 1));
    assert!(r.visible.iter().all(|v| *v == 1), "掩码像素全部落笔（替换或 XOR）");
    assert_eq!(&r.xor, &[0, 1, 0, 0], "仅 alpha=0xFF 打 XOR 标记");
    assert_eq!(&r.bgra[4..8], &[4, 5, 6, 255], "RGB 保留、按不透明存储");

    // 截断（缓冲 < pitch*h=16）→ 错误；未知类型 → 空栅格（不打补丁、不杀流）。
    assert!(render_cursor_shape(4, 4, 1, 16, &[0u8; 15]).is_err());
    let unk = render_cursor_shape(99, 8, 8, 4, &[0u8; 32]).expect("未知类型不报错");
    assert_eq!((unk.w, unk.h), (0, 0));
}

#[test]
fn pointer_snapshot_equality_covers_every_field() {
    use mode_cua_ax::rd_media::capture::PointerSnapshot;
    // 判重键语义：完全相等的快照 = 无新信息（不重合成）；任一字段不同 = 需重合成。
    // 本用例把「判重必须覆盖全部字段」钉成契约——未来加字段若漏进判重键，这里先红。
    let base = PointerSnapshot { x: 10, y: 20, shape: 4096, visible: true };
    assert_eq!(base, PointerSnapshot { x: 10, y: 20, shape: 4096, visible: true });
    assert_ne!(base, PointerSnapshot { x: 11, y: 20, shape: 4096, visible: true });
    assert_ne!(base, PointerSnapshot { x: 10, y: 21, shape: 4096, visible: true });
    assert_ne!(base, PointerSnapshot { x: 10, y: 20, shape: 2048, visible: true });
    assert_ne!(base, PointerSnapshot { x: 10, y: 20, shape: 4096, visible: false });
}
