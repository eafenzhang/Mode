//! 进程内 Win32 夹具窗口（Task 3 建立，Task 4/5 经 `mod common;` 复用）。
//! 顶层窗口内嵌 EDIT + BUTTON，独立线程跑消息泵；`destroy()`（Drop 兜底）
//! 关窗口并 join 线程，避免测试间窗口残留。
//!
//! 相对 brief 草图的适配（均已记录在 task-3 report）：
//! 1. 草图的 `"...\0".into()` → `PCWSTR` 在 windows 0.58 没有对应 `From<&str>` 实现，
//!    统一用 `wide()` 构造 NUL 结尾 UTF-16 并以 `PCWSTR(ptr)` 传入（字符串活到调用结束）。
//! 2. 草图的 `lpfnWndProc: Some(def_window_proc)` 只会转发默认处理，但 destroy 依赖
//!    「WM_DESTROY → PostQuitMessage 结束消息泵」，否则 GetMessageW 永不返回、join 挂死——
//!    窗口过程必须自行处理 WM_DESTROY（brief 注释本身也写了这一语义）。
//! 3. 草图的 `destroy() = DestroyWindow(HWND)` 跨线程调用必然失败（Win32 只允许销毁
//!    本线程创建的窗口），改为 `PostMessageW(WM_CLOSE)`，由窗口线程 DefWindowProc 执行
//!    DestroyWindow → WM_DESTROY → PostQuitMessage。
//! 4. 草图的 `HMENU(1)`：windows 0.58 的 `HMENU(pub *mut c_void)`，整数需先转裸指针。
//! 5. 草图结构体里的 `tx: Option<mpsc::Sender<()>>` 在草图中无任何用途（ready 已由
//!    ready_rx 承担），保留只会 dead_code——移除；Drop + destroy() 语义不变。
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::*;

/// NUL 结尾的 UTF-16 序列；调用方保证其存活期覆盖 Win32 调用。
fn wide(s: &str) -> Vec<u16> {
  s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 夹具窗口过程：WM_DESTROY 时退出消息泵，其余走默认处理。
unsafe extern "system" fn fixture_wnd_proc(
  hwnd: HWND,
  msg: u32,
  wparam: WPARAM,
  lparam: LPARAM,
) -> LRESULT {
  if msg == WM_DESTROY {
    PostQuitMessage(0);
    return LRESULT(0);
  }
  DefWindowProcW(hwnd, msg, wparam, lparam)
}

pub struct Fixture {
  pub hwnd: isize,
  join: Option<JoinHandle<()>>,
}

impl Fixture {
  /// 销毁窗口并回收夹具线程（幂等；测试正常路径显式调用，panic 时 Drop 兜底）。
  pub fn destroy(mut self) {
    self.shutdown();
  }

  fn shutdown(&mut self) {
    if self.join.is_none() {
      return; // 已销毁
    }
    // 跨线程销毁：PostMessage(WM_CLOSE) → 窗口线程 DefWindowProc → DestroyWindow
    // → 我们的窗口过程收 WM_DESTROY → PostQuitMessage → 消息泵退出。
    unsafe {
      let _ = PostMessageW(
        HWND(self.hwnd as usize as *mut _),
        WM_CLOSE,
        WPARAM(0),
        LPARAM(0),
      );
    }
    if let Some(join) = self.join.take() {
      let _ = join.join();
    }
  }
}

impl Drop for Fixture {
  fn drop(&mut self) {
    self.shutdown();
  }
}

/// 建一个顶层窗口，内嵌 EDIT + BUTTON + RichEdit50W；独立线程跑消息泵；返回 hwnd。
/// （Task 5 追加 RichEdit：单行 EDIT 实测只暴露 UIA ValuePattern，`select_text` 的
/// TextPattern 路径需要一个原生 UIA 文本提供者——msftedit.dll 的 RichEdit50W（实测
/// 暴露 TextPattern；riched20.dll 的 RichEdit20W 不暴露）。既有用例按 kind/value
/// 定位控件，新增子窗口不影响它们的断言。）
pub fn spawn_fixture() -> Fixture {
  let (ready_tx, ready_rx) = mpsc::channel();
  let class = wide("ModeCuaTest");
  let title = wide("fixture");
  let edit_cls = wide("EDIT");
  let edit_text = wide("hello");
  let btn_cls = wide("BUTTON");
  let btn_text = wide("press me");

  let join = std::thread::spawn(move || unsafe {
    // 类名第二次注册（同进程多夹具并行）返回 0 但类已存在，创建窗口不受影响。
    let _ = RegisterClassW(&WNDCLASSW {
      lpszClassName: PCWSTR(class.as_ptr()),
      lpfnWndProc: Some(fixture_wnd_proc),
      hInstance: GetModuleHandleW(None).unwrap().into(),
      ..Default::default()
    });
    let hwnd = CreateWindowExW(
      WINDOW_EX_STYLE::default(),
      PCWSTR(class.as_ptr()),
      PCWSTR(title.as_ptr()),
      WS_OVERLAPPEDWINDOW,
      0,
      0,
      320,
      240,
      None,
      None,
      None,
      None,
    )
    .unwrap();
    CreateWindowExW(
      WINDOW_EX_STYLE::default(),
      PCWSTR(edit_cls.as_ptr()),
      PCWSTR(edit_text.as_ptr()),
      WS_CHILD | WS_VISIBLE,
      10,
      10,
      200,
      24,
      hwnd,
      // 子窗口 ID 经 HMENU 参数传递：是句柄值不是真指针，用 without_provenance_mut
      // （无 provenance、保留地址）而非整数裸转（clippy manual_dangling_ptr）。
      HMENU(std::ptr::without_provenance_mut(1)),
      None,
      None,
    )
    .unwrap();
    CreateWindowExW(
      WINDOW_EX_STYLE::default(),
      PCWSTR(btn_cls.as_ptr()),
      PCWSTR(btn_text.as_ptr()),
      WS_CHILD | WS_VISIBLE,
      10,
      50,
      100,
      28,
      hwnd,
      HMENU(std::ptr::without_provenance_mut(2)),
      None,
      None,
    )
    .unwrap();
    // Task 5：RichEdit50W = UIA TextPattern 宿主（select_text 用例）。类由 msftedit.dll
    // 注册：LoadLibrary 引用计数常驻到进程结束；创建失败直接 panic（夹具不完整即失败）。
    // WHY msftedit 而非 riched20 的 RichEdit20W：pattern 探针实测 RichEdit20W 不暴露
    // TextPattern（GetCurrentPatternAs 失败），RichEdit50W 暴露（text=true）。
    let rich_cls = wide("RichEdit50W");
    let rich_dll = wide("msftedit.dll");
    let _ = LoadLibraryW(PCWSTR(rich_dll.as_ptr()));
    CreateWindowExW(
      WINDOW_EX_STYLE::default(),
      PCWSTR(rich_cls.as_ptr()),
      PCWSTR(wide("rich text").as_ptr()),
      WS_CHILD | WS_VISIBLE,
      10,
      90,
      200,
      40,
      hwnd,
      HMENU(std::ptr::without_provenance_mut(3)),
      None,
      None,
    )
    .unwrap();
    let _ = ShowWindow(hwnd, SW_SHOW); // 返回值 must_use，显示失败不影响夹具语义
    let _ = ready_tx.send(hwnd.0 as usize as isize);
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).into() {
      let _ = TranslateMessage(&msg);
      let _ = DispatchMessageW(&msg);
    }
  });
  let hwnd = ready_rx.recv().expect("fixture window ready");
  Fixture {
    hwnd,
    join: Some(join),
  }
}
