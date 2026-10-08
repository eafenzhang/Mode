mod common;

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
