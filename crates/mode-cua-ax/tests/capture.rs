mod common;

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use mode_cua_ax::capture::should_blit_first;
use mode_cua_ax::*;
use std::ffi::c_void;
use windows::Win32::Foundation::HWND;

#[test]
fn capture_fixture_window_png_nonzero() {
  let fix = common::spawn_fixture();
  let r = capture::capture(Some(fix.hwnd as u32), None, false).expect("capture");
  assert!(r.data.len() > 1024);
  assert_eq!(&r.data[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
  assert!(r.width > 0 && r.height > 0);
  fix.destroy();
}

#[test]
fn region_outside_window_is_clamped() {
  let fix = common::spawn_fixture();
  let r = capture::capture(Some(fix.hwnd as u32), Some(vec![-50, -50, 10_000, 10_000]), false)
    .expect("capture");
  assert!(r.clamped);
  fix.destroy();
}

#[test]
fn live_fixture_window_takes_printwindow_path() {
  let fix = common::spawn_fixture();
  let hwnd = HWND(fix.hwnd as usize as *mut c_void);
  // 活夹具窗口消息泵正常（非挂起）→ 路由预检必须放行 PrintWindow 主路径；
  // 一旦误判为挂起，截图会静默降级 BitBlt（丢非客户区/被遮挡内容），此断言守住路由。
  assert!(!should_blit_first(hwnd));
  fix.destroy();
}

// ---- Task 6：帧预算质量阶梯（纯函数面，不走 GDI） ----

/// base64 编码长度（`4 * ceil(n/3)`，与 JS `bytes.toString("base64")` 等长）。
/// 预算口径 = base64 字符数（与 frame-contract.js 的 `Buffer.byteLength` 比较一致）。
fn b64_len(bytes: usize) -> usize {
  4 * bytes.div_ceil(3)
}

/// 确定性 LCG 噪声（Knuth MMIX 乘数）：不可压缩输入，压缩结果跨运行可复现。
fn lcg_noise_rgb(width: u32, height: u32, seed: u64) -> Vec<u8> {
  let mut state = seed;
  let len = width as usize * height as usize * 3;
  let mut out = Vec::with_capacity(len);
  for _ in 0..len {
    state = state
      .wrapping_mul(6_364_136_223_846_793_005)
      .wrapping_add(1_442_695_040_888_963_407);
    out.push((state >> 33) as u8);
  }
  out
}

/// RGB → PNG（预算门与失败分支的输入都用真编码字节，不 mock）。
fn encode_png(rgb: &[u8], width: u32, height: u32) -> Vec<u8> {
  let mut png = Vec::new();
  PngEncoder::new(&mut png)
    .write_image(rgb, width, height, ExtendedColorType::Rgb8)
    .expect("PNG 编码");
  png
}

#[test]
fn fit_frame_budget_reencodes_overbudget_noise_to_jpeg() {
  const W: u32 = 320;
  const H: u32 = 320;
  let png = encode_png(&lcg_noise_rgb(W, H, 0xC0FF_EE01), W, H);
  // 前置：噪声 PNG 的 base64 必须确实超预算，否则测不到阶梯。
  assert!(
    b64_len(png.len()) > 200 * 1024,
    "前置条件失败：噪声 PNG 未超预算（b64={}）",
    b64_len(png.len())
  );
  let (out, mime) = capture::fit_frame_budget(&png, W, H).expect("质量阶梯应产出限内帧");
  assert_eq!(mime, "image/jpeg");
  assert!(
    b64_len(out.len()) <= 200 * 1024,
    "阶梯输出必须 ≤200KiB（b64={}）",
    b64_len(out.len())
  );
  assert_eq!(&out[..2], &[0xFF, 0xD8], "输出应为 JPEG（SOI 标记）");
  let decoded = image::load_from_memory(&out).expect("JPEG 应可解码");
  assert_eq!(
    (decoded.width(), decoded.height()),
    (W, H),
    "重编码不得改变几何尺寸"
  );
}

#[test]
fn fit_frame_budget_underbudget_png_returned_unchanged() {
  const W: u32 = 32;
  const H: u32 = 32;
  let png = encode_png(&lcg_noise_rgb(W, H, 7), W, H);
  assert!(b64_len(png.len()) <= 200 * 1024, "前置条件失败：PNG 超预算");
  let (out, mime) = capture::fit_frame_budget(&png, W, H).expect("预算内恒等返回");
  assert_eq!(mime, "image/png");
  assert_eq!(out, png, "预算内必须逐字节原样返回（不重编码）");
}

#[test]
fn fit_frame_budget_exhausted_ladder_returns_internal_error() {
  const W: u32 = 320;
  const H: u32 = 320;
  let png = encode_png(&lcg_noise_rgb(W, H, 99), W, H);
  assert!(b64_len(png.len()) > 200 * 1024, "前置条件失败：PNG 未超预算");
  // 注入空质量序列 = 阶梯一次尝试都没有 → 必然失败分支（不依赖编码器实现细节）。
  let err = capture::fit_frame_budget_with_qualities(&png, W, H, &[])
    .expect_err("空质量序列必然超预算");
  assert_eq!(err.code, "internal");
  assert_eq!(err.message, "capture exceeds frame budget");
}

#[test]
fn fit_frame_budget_real_noise_beyond_ladder_returns_internal_error() {
  const W: u32 = 1024;
  const H: u32 = 1024;
  let png = encode_png(&lcg_noise_rgb(W, H, 4242), W, H);
  assert!(b64_len(png.len()) > 200 * 1024, "前置条件失败：PNG 未超预算");
  // 真实最坏输入：1MP 全噪声即使用尽生产阶梯（最低 q40）仍超预算。
  let err = capture::fit_frame_budget(&png, W, H)
    .expect_err("1MP 全噪声即使降到 q40 仍超预算");
  assert_eq!(err.code, "internal");
  assert_eq!(err.message, "capture exceeds frame budget");
}

#[test]
fn capture_reports_identity_png_mime_type() {
  // 接线守卫：夹具小窗走恒等路径，capture 出参必须携带 image/png。
  let fix = common::spawn_fixture();
  let r = capture::capture(Some(fix.hwnd as u32), None, false).expect("capture");
  assert_eq!(r.mime_type, "image/png");
  fix.destroy();
}
