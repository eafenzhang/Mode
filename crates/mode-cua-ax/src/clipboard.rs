//! `paste` 用的系统剪贴板：CF_UNICODETEXT 保存 / 写入 / 还原。
//!
//! brief 语义：保存 CF_UNICODETEXT → 写入新文本 → Ctrl+V → 短 settle → 还原剪贴板。
//! `ClipboardBackup` 是 RAII 守卫：无论成功返回还是错误提前返回/panic 展开，Drop 都会
//! 尽力还原（paste 的「还原剪贴板」在 panic 路径同样成立）。
//!
//! 已知边界（brief 明示只处理 CF_UNICODETEXT）：用户剪贴板上的**非文本格式**（图片等）
//! 在本轮 paste 中会丢失——保存面就是文本，这是 brief 定的范围，不是遗漏。
use crate::error::{AxError, AxResult};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
  CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
  SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// 预定义格式 CF_UNICODETEXT = 13（CLIPBOARD_FORMAT(13)；Get/SetClipboardData 收 u32）。
const UNICODE_TEXT: u32 = CF_UNICODETEXT.0 as u32;

/// 单次读取上限（UTF-16 码元）：防坏剪贴板无 NUL 时无限扫描。
const MAX_UNITS: usize = 16 * 1024 * 1024;

/// OpenClipboard 争用重试预算（别的进程短暂占用是常态）。
fn open_with_retry() -> AxResult<()> {
  let deadline = Instant::now() + Duration::from_millis(200);
  loop {
    // unsafe 依据（块级）：OpenClipboard FFI；None = 不关联窗口属主。
    if unsafe { OpenClipboard(None) }.is_ok() {
      return Ok(());
    }
    if Instant::now() >= deadline {
      return Err(AxError::new(
        "internal",
        "OpenClipboard 失败（剪贴板被其他进程持续占用）",
      ));
    }
    std::thread::sleep(Duration::from_millis(20));
  }
}

/// 读 CF_UNICODETEXT；格式不存在 → Ok(None)（不报错——「原本就没有文本」是合法状态）。
pub fn read_text() -> AxResult<Option<String>> {
  open_with_retry()?;
  // unsafe 依据（块级）：GetClipboardData/GlobalLock/GlobalUnlock/CloseClipboard 均为 FFI；
  // 句柄有效性由「OpenClipboard 成功且未 Close」的窗口期保证。
  unsafe {
    let got = if IsClipboardFormatAvailable(UNICODE_TEXT).is_ok() {
      match GetClipboardData(UNICODE_TEXT) {
        Ok(handle) => read_hglobal(handle),
        // 格式在但取不到（竞态被清空）→ 按「无文本」处理。
        Err(_) => Ok(None),
      }
    } else {
      Ok(None)
    };
    let _ = CloseClipboard();
    got
  }
}

/// HGLOBAL → String（GlobalLock → 扫到 NUL → 解码 → GlobalUnlock）。
unsafe fn read_hglobal(handle: HANDLE) -> AxResult<Option<String>> {
  let mem = HGLOBAL(handle.0);
  let ptr = GlobalLock(mem);
  if ptr.is_null() {
    return Ok(None);
  }
  let mut len = 0usize;
  while len < MAX_UNITS {
    // ptr 指向系统持有的可移动内存块，读到 NUL 为止。
    let unit = *(ptr as *const u16).add(len);
    if unit == 0 {
      break;
    }
    len += 1;
  }
  let slice = std::slice::from_raw_parts(ptr as *const u16, len);
  let text = String::from_utf16_lossy(slice);
  let _ = GlobalUnlock(mem);
  Ok(Some(text))
}

/// 写 CF_UNICODETEXT（先 EmptyClipboard 交出属主权）。
pub fn write_text(text: &str) -> AxResult<()> {
  open_with_retry()?;
  let out = unsafe { write_locked(text) };
  let _ = unsafe { CloseClipboard() };
  out
}

/// 调用方已 OpenClipboard。
unsafe fn write_locked(text: &str) -> AxResult<()> {
  EmptyClipboard().map_err(|e| {
    AxError::new("internal", format!("EmptyClipboard 失败: {e}"))
  })?;
  let wide = crate::input::wide_z(text);
  let bytes = wide.len() * 2;
  let mem = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| {
    AxError::new("internal", format!("GlobalAlloc 剪贴板块失败: {e}"))
  })?;
  let ptr = GlobalLock(mem);
  if ptr.is_null() {
    let _ = GlobalFree(mem);
    return Err(AxError::new("internal", "GlobalLock 剪贴板块失败"));
  }
  // 码元级拷贝（wide 含结尾 NUL）。
  std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, ptr as *mut u8, bytes);
  // 解锁失败不致命（系统在 SetClipboardData 后接管所有权）。
  let _ = GlobalUnlock(mem);
  if let Err(e) = SetClipboardData(UNICODE_TEXT, HANDLE(mem.0)) {
    // SetClipboardData 失败 → 块仍归我们，释放；成功则系统接管、绝不可再 free。
    let _ = GlobalFree(mem);
    return Err(AxError::new("internal", format!("SetClipboardData 失败: {e}")));
  }
  Ok(())
}

/// 清空剪贴板（还原到「原本无文本」的状态用）。
fn clear() -> AxResult<()> {
  open_with_retry()?;
  let out = unsafe { EmptyClipboard() }
    .map_err(|e| AxError::new("internal", format!("EmptyClipboard 失败: {e}")));
  let _ = unsafe { CloseClipboard() };
  out
}

/// paste 的剪贴板备份：构造时抓取 CF_UNICODETEXT 快照，`restore`/Drop 还原。
pub struct ClipboardBackup {
  text: Option<String>,
}

impl ClipboardBackup {
  /// 抓取当前 CF_UNICODETEXT（无文本格式记为 None）。
  pub fn capture() -> AxResult<Self> {
    Ok(Self {
      text: read_text()?,
    })
  }

  /// 还原到备份（尽力而为：还原失败不覆盖已成功的 paste 结果——粘贴已发生，
  /// 报失败会让上层误判动作没送达；失败详情无法上抛的现实按已知边界记录）。
  pub fn restore(&self) {
    match &self.text {
      Some(t) => {
        let _ = write_text(t);
      }
      None => {
        let _ = clear();
      }
    }
  }
}

impl Drop for ClipboardBackup {
  fn drop(&mut self) {
    // panic/早退路径同样还原（RAII；restore 内部不 panic）。
    self.restore();
  }
}
