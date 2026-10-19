//! 剪贴板面：`rdReadClipboardText`（LR-3，被控/主控剪贴板同步的读取口）。
//!
//! 与 Electron main 的 `clipboard.readText()` 同语义：读 CF_UNICODETEXT，无文本返回空串。
//! 非 Windows 平台返回空串（远程桌面本身也仅 Windows 支持）。
//!
//! 为什么放在壳：读系统剪贴板是 OS 能力，Host 不直接碰平台 API（D7）；
//! 主控 renderer 轮询本命令后把文本上行到媒体 WS。

/// `IPlatformService.rdReadClipboardText(): Promise<string>`（无文本 = 空串）。
///
/// 读取失败（剪贴板被长期占用、非 Windows）不抛错：剪贴板争用是常态，
/// 抛错会让 renderer 每拍都进 catch。空串在协议里就是「本拍无变化」。
#[tauri::command(rename = "rdReadClipboardText")]
pub fn rd_read_clipboard_text() -> Result<String, String> {
    #[cfg(windows)]
    {
        Ok(read_windows_clipboard_text())
    }
    #[cfg(not(windows))]
    {
        Ok(String::new())
    }
}

#[cfg(windows)]
fn read_windows_clipboard_text() -> String {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    let format = CF_UNICODETEXT as u32;
    // OpenClipboard 争用重试：别的进程短暂占用是常态（与 CUA helper 同口径）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    loop {
        // unsafe 依据：OpenClipboard 是 FFI；None 表示不关联窗口属主。
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return String::new();
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // unsafe 依据：句柄有效性由「OpenClipboard 成功且未 Close」的窗口期保证；
    // 读取上界用 GlobalSize，绝不越过分配读内存。
    unsafe {
        let mut out = String::new();
        if IsClipboardFormatAvailable(format) != 0 {
            // windows-sys 0.61 里 GetClipboardData 的返回值与 GlobalLock 的 HGLOBAL 同型
            // （都是 `*mut c_void`），因此直接传递，不做转换。
            let mem = GetClipboardData(format);
            if !mem.is_null() {
                let ptr = GlobalLock(mem);
                if !ptr.is_null() {
                    let alloc_bytes = GlobalSize(mem);
                    let max_units = alloc_bytes / 2;
                    let wide = ptr as *const u16;
                    let mut len = 0usize;
                    while len < max_units {
                        if *wide.add(len) == 0 {
                            out = String::from_utf16_lossy(std::slice::from_raw_parts(wide, len));
                            break;
                        }
                        len += 1;
                    }
                    let _ = GlobalUnlock(mem);
                }
            }
        }
        let _ = CloseClipboard();
        out
    }
}
