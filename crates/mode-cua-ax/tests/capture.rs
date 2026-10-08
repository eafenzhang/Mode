mod common;

use mode_cua_ax::*;

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
