//! 锁屏探测（Task 6）：`OpenInputDesktop` 是唯一读操作——锁屏时输入桌面不可打开，
//! 打开成功/失败即 unlocked/locked，句柄当帧关闭，零驻留。
use windows::Win32::Foundation::{FALSE, GENERIC_READ};
use windows::Win32::System::StationsAndDesktops::{
  CloseDesktop, OpenInputDesktop, HDESK, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS,
};

use crate::error::AxResult;
use crate::ScreenProbeNapi;

/// HDESK RAII：按 API 契约用 `CloseDesktop`（不是 `CloseHandle`——桌面句柄不是内核对象，
/// CloseHandle 语义不符，Win32 文档点名成对使用 CloseDesktop）。
struct CloseDesktopOnDrop(HDESK);

impl Drop for CloseDesktopOnDrop {
  fn drop(&mut self) {
    // 关闭失败只可能是句柄已失效，探测结果不受影响。
    let _ = unsafe { CloseDesktop(self.0) };
  }
}

/// 探测当前会话是否锁屏：`OpenInputDesktop(0, FALSE, GENERIC_READ)` 失败即锁屏
/// （brief 口径；成功分支的 HDESK 由守卫在返回前按契约关闭）。
pub fn probe() -> AxResult<ScreenProbeNapi> {
  let desk = unsafe {
    OpenInputDesktop(
      DESKTOP_CONTROL_FLAGS(0),
      FALSE,
      DESKTOP_ACCESS_FLAGS(GENERIC_READ.0),
    )
  };
  match desk {
    Ok(h) => {
      // 守卫活到函数尾：关闭失败只可能是句柄已失效，不值得因此失败。
      let _guard = CloseDesktopOnDrop(h);
      Ok(ScreenProbeNapi { locked: false })
    }
    Err(_) => Ok(ScreenProbeNapi { locked: true }),
  }
}
