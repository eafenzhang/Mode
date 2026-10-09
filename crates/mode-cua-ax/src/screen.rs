//! 锁屏探测（Task 6）：`OpenInputDesktop` 是唯一读操作——锁屏时输入桌面不可打开，
//! 打开成功/失败即 unlocked/locked，句柄当帧关闭（复用 input 的 CloseOnDrop 模式）。
use windows::Win32::Foundation::{FALSE, GENERIC_READ, HANDLE};
use windows::Win32::System::StationsAndDesktops::{
  OpenInputDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS,
};

use crate::error::AxResult;
use crate::input::CloseOnDrop;
use crate::ScreenProbeNapi;

/// 探测当前会话是否锁屏：`OpenInputDesktop(0, FALSE, GENERIC_READ)` 失败即锁屏
/// （brief 口径；不引入任何驻留句柄——成功分支的 HDESK 由守卫在返回前关闭）。
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
      let _guard = CloseOnDrop(HANDLE(h.0));
      Ok(ScreenProbeNapi { locked: false })
    }
    Err(_) => Ok(ScreenProbeNapi { locked: true }),
  }
}
