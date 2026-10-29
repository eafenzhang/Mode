//! 进程存活检测（废弃锁回收要用）。
//!
//! 与 TS `process.kill(pid, 0)` 同义：只判断「这个 pid 现在有没有活进程」，不看权限之外的属性。
//! 拿不准时一律返回 `true`（宁可不回收，也不误删别人正在用的锁）。

#[cfg(windows)]
pub(crate) fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// `STILL_ACTIVE` 在 windows-sys 里没有导出常量，取值固定为 259。
    const STILL_ACTIVE: u32 = 259;

    if pid == 0 {
        return false;
    }
    // SAFETY: OpenProcess/GetExitCodeProcess/CloseHandle 都是只读查询，句柄在本函数内关闭。
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // 打不开通常是「进程不存在」或「权限不足」；后者按存活处理更安全。
            // 用 GetLastError 区分：ERROR_INVALID_PARAMETER(87) 才是「没有这个进程」。
            let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return code != 87;
        }
        let mut exit_code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        if ok == 0 {
            return true;
        }
        exit_code == STILL_ACTIVE
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    match std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
    {
        Ok(status) => status.success(),
        // 拿不准时不回收。
        Err(_) => true,
    }
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn process_alive(_pid: u32) -> bool {
    true
}
