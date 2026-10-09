//! 应用与窗口枚举（Task 2 的纯逻辑面）：napi 包装只在 `lib.rs` 做错误转码。
use crate::error::{AxError, AxResult};
use crate::{AppInfoNapi, WindowRowNapi};
use std::collections::HashSet;
use windows::core::PWSTR;
use windows::Win32::Foundation::{
  CloseHandle, BOOL, ERROR_SUCCESS, HANDLE, HWND, LPARAM, RECT, TRUE,
};
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Threading::{
  GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
  PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
  EnumWindows, GetAncestor, GetForegroundWindow, GetWindowRect, GetWindowTextLengthW,
  GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, GA_PARENT, HWND_MESSAGE,
};

/// 枚举回调的可变上下文（回调不能捕获环境，经 LPARAM 传指针）。
struct EnumCtx {
  out: Vec<HWND>,
  own_pid: u32,
}

unsafe extern "system" fn collect_visible(hwnd: HWND, lparam: LPARAM) -> BOOL {
  let ctx = &mut *(lparam.0 as *mut EnumCtx);
  if IsWindowVisible(hwnd).as_bool() {
    let pid = window_pid(hwnd);
    // 「非自身消息窗」：本进程自己的消息窗不是可观察的应用窗口（消息窗本就不会被
    // EnumWindows 返回，这里按 parent==HWND_MESSAGE 防御性过滤；本进程的真实可见窗口
    // 不受影响，后续任务的进程内测试夹具窗口仍会被列出）。
    let own_message_window = pid == ctx.own_pid && GetAncestor(hwnd, GA_PARENT) == HWND_MESSAGE;
    if pid != 0 && !own_message_window {
      ctx.out.push(hwnd);
    }
  }
  TRUE
}

/// 全部可见顶层窗口（z 序）。窗口在枚举与读取之间消亡属正常竞态，逐窗降级处理。
/// （Task 6 复用：launch 的启动前快照与 pid 回填轮询同一口径。）
pub(crate) fn visible_top_level_windows() -> AxResult<Vec<HWND>> {
  let mut ctx = EnumCtx {
    out: Vec::new(),
    own_pid: unsafe { GetCurrentProcessId() },
  };
  unsafe {
    EnumWindows(Some(collect_visible), LPARAM(&mut ctx as *mut EnumCtx as isize))
      .map_err(|e| AxError::internal(format!("EnumWindows failed: {e}")))?;
  }
  Ok(ctx.out)
}

/// 窗口所属进程 pid；取不到返回 0（调用方按无效处理）。（Task 6 复用：回填轮询。）
pub(crate) fn window_pid(hwnd: HWND) -> u32 {
  let mut pid = 0u32;
  unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
  pid
}

/// 当前前台窗口所属进程 pid；无前台窗口为 None。（Task 6 复用：launch 的 active 字段。）
pub(crate) fn foreground_pid() -> Option<u32> {
  unsafe {
    let hwnd = GetForegroundWindow();
    if hwnd.is_invalid() {
      return None;
    }
    let pid = window_pid(hwnd);
    (pid != 0).then_some(pid)
  }
}

/// 进程查询句柄（PROCESS_QUERY_LIMITED_INFORMATION），析构即关闭，杜绝早退泄漏。
struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
  fn drop(&mut self) {
    // 关闭失败只可能是句柄已失效，枚举路径不值得因此失败。
    let _ = unsafe { CloseHandle(self.0) };
  }
}

/// 打开进程查询句柄；受保护进程等打开失败返回 None（该进程行降级为 name/bundleId=null）。
fn open_query_process(pid: u32) -> Option<ProcessHandle> {
  let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
  Some(ProcessHandle(handle))
}

/// 进程显示名 = exe 路径末段文件名（如 `explorer.exe`）；读取失败为 None。
fn process_exe_name(handle: HANDLE) -> Option<String> {
  unsafe {
    // QueryFullProcessImageNameW 的缓冲上限为 32767 字符（扩展路径），一次给足避免二次调用。
    let mut buf = vec![0u16; 32768];
    let mut size = buf.len() as u32;
    QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size)
      .ok()?;
    let path = String::from_utf16_lossy(&buf[..size as usize]);
    let name = path.rsplit(['\\', '/']).next()?;
    (!name.is_empty()).then(|| name.to_string())
  }
}

/// AUMID（应用用户模型 ID）；读取失败、返回非 success 或空串都视为「取不到」→ None。
fn process_aumid(handle: HANDLE) -> Option<String> {
  unsafe {
    let mut buf = vec![0u16; 512];
    let mut len = buf.len() as u32;
    let status = GetApplicationUserModelId(handle, &mut len, PWSTR(buf.as_mut_ptr()));
    if status != ERROR_SUCCESS || len == 0 {
      return None;
    }
    // len 的计数口径（含/不含终止 NUL）在 SDK 文档与实现间有出入，统一按尾部 NUL 裁剪；
    // 裁剪后为空说明该进程没有 AUMID。
    let end = (len as usize).min(buf.len());
    let aumid = String::from_utf16_lossy(&buf[..end]);
    let aumid = aumid.trim_end_matches('\0');
    (!aumid.is_empty()).then(|| aumid.to_string())
  }
}

/// 按 pid 取 exe 文件名（Task 6 launch 回填用）；打开/读取失败 → None。
pub(crate) fn exe_name_for_pid(pid: u32) -> Option<String> {
  let handle = open_query_process(pid)?;
  process_exe_name(handle.0)
}

/// 按 pid 取 AUMID（Task 6 launch 尽力回填用）；打开/读取失败 → None。
pub(crate) fn aumid_for_pid(pid: u32) -> Option<String> {
  let handle = open_query_process(pid)?;
  process_aumid(handle.0)
}

/// 标题读取语义：读到文本 → `Some(文本)`；读到但为空 → `Some("")`；
/// 声称有长度却读不出来（失败/竞态）→ `None`——空串只保留给「读到但为空」。
fn window_title(hwnd: HWND) -> Option<String> {
  unsafe {
    let len = GetWindowTextLengthW(hwnd);
    let mut buf = vec![0u16; len.max(0) as usize + 1];
    let n = GetWindowTextW(hwnd, &mut buf);
    if n > 0 {
      Some(String::from_utf16_lossy(&buf[..n as usize]))
    } else if len == 0 {
      Some(String::new())
    } else {
      None
    }
  }
}

/// 枚举桌面上有可见顶层窗口的进程（一个进程一行，z 序在前的应用靠前）。
pub fn list_apps() -> AxResult<Vec<AppInfoNapi>> {
  let hwnds = visible_top_level_windows()?;
  let foreground = foreground_pid();
  let mut seen: HashSet<u32> = HashSet::new();
  let mut out = Vec::new();
  for hwnd in hwnds {
    let pid = window_pid(hwnd);
    if pid == 0 || !seen.insert(pid) {
      continue;
    }
    let (name, bundle_id) = match open_query_process(pid) {
      Some(handle) => (process_exe_name(handle.0), process_aumid(handle.0)),
      None => (None, None),
    };
    out.push(AppInfoNapi {
      pid,
      name,
      bundle_id,
      active: foreground == Some(pid),
    });
  }
  Ok(out)
}

/// 枚举可见顶层窗口；`pid` 过滤指定进程。`main` = 该进程第一个顶层窗口（z 序），
/// `focused` = 当前前台窗口，`onscreen` 恒 true（Win32 无 CoreGraphics 语义）。
pub fn list_windows(pid: Option<u32>) -> AxResult<Vec<WindowRowNapi>> {
  let hwnds = visible_top_level_windows()?;
  let focused = unsafe { GetForegroundWindow() };
  let mut main_seen: HashSet<u32> = HashSet::new();
  let mut rows = Vec::new();
  for hwnd in hwnds {
    let wpid = window_pid(hwnd);
    if let Some(filter) = pid {
      if wpid != filter {
        continue;
      }
    }
    let mut rect = RECT::default();
    // 窗口可能在枚举与读取之间消亡：读不到矩形的窗口丢弃，不产出残缺行。
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
      continue;
    }
    rows.push(WindowRowNapi {
      // HWND 在 64 位进程里仍是 32 位有效窗口句柄，取低 32 位作为跨进程窗口 id。
      window_id: hwnd.0 as usize as u32,
      pid: wpid,
      title: window_title(hwnd),
      // bounds 口径与观察层一致：[x, y, width, height]（非 RECT 的 left/top/right/bottom）。
      bounds: vec![rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top],
      main: main_seen.insert(wpid),
      focused: hwnd == focused,
      onscreen: true,
    });
  }
  Ok(rows)
}
