//! Task 6：应用启动（三段解析）与锁屏探测。
//!
//! brief 三用例 + 两个强化（invalid_request 校验、Start Menu 递归扫描/IShellLink 目标读取）。
//! `launch_search_path_finds_notepad` 会真实启动 notepad.exe，测试用 Drop 守卫收割，
//! panic 路径同样不遗留进程。
use mode_cua_ax::{launch, screen};
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
  OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
  PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

/// 读取已打开进程的 exe 文件名（末段，如 `notepad.exe`）；失败为 None。
fn query_exe_name(handle: HANDLE) -> Option<String> {
  // QueryFullProcessImageNameW 缓冲上限 32767 字符，一次给足（同 apps.rs 口径）。
  let mut buf = vec![0u16; 32768];
  let mut size = buf.len() as u32;
  unsafe {
    QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size)
  }
  .ok()?;
  let path = String::from_utf16_lossy(&buf[..size as usize]);
  Some(path.rsplit(['\\', '/']).next()?.to_string())
}

/// 收割本次启动的 notepad：双保险——只有进程名确认为 notepad（或名字读不出来、
/// 但 pid 来自本次 launch 返回值）才 TerminateProcess；打开失败视为已退出。
fn reap_notepad(pid: u32) {
  if pid == 0 {
    eprintln!("launch 未回填 pid（0），无法收割 notepad");
    return;
  }
  let handle = unsafe {
    OpenProcess(
      PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
      false,
      pid,
    )
  };
  let Ok(handle) = handle else {
    return; // 进程已退出，无需收割
  };
  match query_exe_name(handle) {
    Some(name) if !name.to_lowercase().contains("notepad") => {
      eprintln!("pid {pid} 实为 {name}（非 notepad），不收割");
    }
    _ => {
      let _ = unsafe { TerminateProcess(handle, 1) };
    }
  }
  let _ = unsafe { CloseHandle(handle) };
}

/// notepad 收割守卫：测试正常结束与 panic（assert 失败）都会 Drop。
struct ReapNotepad {
  pid: u32,
}

impl Drop for ReapNotepad {
  fn drop(&mut self) {
    reap_notepad(self.pid);
  }
}

// ————— brief Step 1 三用例（原文断言不改；notepad 用例插一行收割守卫） —————

#[test]
fn screen_probe_reports_unlocked_in_ci() {
  assert!(!screen::probe().unwrap().locked);
}

#[test]
fn launch_search_path_finds_notepad() {
  let app = launch::launch(&launch::Target::Name("notepad".into())).unwrap();
  // 真实启动的真实进程，测试尾部（含 panic 路径）收割，不留进程。
  let _reap = ReapNotepad { pid: app.pid };
  eprintln!("launch resolved: name={:?} pid={} bundle={:?}", app.name, app.pid, app.bundle_id);
  assert!(app.name.unwrap().to_lowercase().contains("notepad"));
  // 回填语义（brief Step 3）：返回的 pid 下确有可见窗口——不只是 spawn 出来的数字。
  let wins = mode_cua_ax::apps::list_windows(Some(app.pid)).unwrap();
  assert!(!wins.is_empty(), "回填 pid {pid} 下没有窗口", pid = app.pid);
}

#[test]
fn launch_unknown_name_is_launch_failed() {
  assert_eq!(
    launch::launch(&launch::Target::Name("definitely-not-an-app-xyz".into()))
      .unwrap_err()
      .code,
    "launch_failed"
  );
}

// ————— 强化：参数非法 → invalid_request；Start Menu 递归扫描 + IShellLinkW 目标读取 —————

#[test]
fn launch_empty_name_is_invalid_request() {
  let err = launch::launch(&launch::Target::Name("   ".into())).unwrap_err();
  assert_eq!(err.code, "invalid_request");
}

#[test]
fn launch_unknown_bundle_id_is_launch_failed() {
  // 点状值 → 第 1 段 ActivateApplication 尝试 → 失败 → 第 2/3 段回退 → 全失败。
  let err = launch::launch(&launch::Target::BundleId("definitely.not-an-app-xyz".into()))
    .unwrap_err();
  assert_eq!(err.code, "launch_failed");
}

#[test]
fn target_from_normalizes_request_fields() {
  // 点状 bundleId 即便带 name 也以 AUMID 段优先（brief 解析顺序首位）。
  assert!(matches!(
    launch::target_from(Some("notepad".into()), Some("X.Y_App!App".into())).unwrap(),
    launch::Target::BundleId(_)
  ));
  // 裸字符串（无 name）按名字解析（brief：裸字符串按 bundleId 传入）。
  assert!(matches!(
    launch::target_from(None, Some("notepad".into())).unwrap(),
    launch::Target::Name(n) if n == "notepad"
  ));
  // 有 name、bundleId 非点状 → 走 name 段。
  assert!(matches!(
    launch::target_from(Some("calc".into()), Some("nodot".into())).unwrap(),
    launch::Target::Name(n) if n == "calc"
  ));
  // 两者皆空（含空白）→ invalid_request。
  let err = launch::target_from(None, Some("   ".into())).unwrap_err();
  assert_eq!(err.code, "invalid_request");
}

#[test]
fn start_menu_scan_covers_subdirectories() {
  let links = launch::scan_start_menu_links();
  if links.is_empty() {
    // 极简环境（无 Start Menu 根 / 无任何 .lnk）：如实跳过，不断言假阳性。
    eprintln!("SKIP: 本机 Start Menu 无任何 .lnk，跳过子目录覆盖断言");
    return;
  }
  // 递归覆盖证据：至少一条链接位于 Start Menu 根的子目录（Programs\\...）深处。
  let in_subdir = links.iter().any(|l| {
    let full = l.path.to_string_lossy();
    let after_root = full.split("Start Menu").nth(1).unwrap_or_default();
    after_root.trim_start_matches('\\').contains('\\')
  });
  assert!(in_subdir, "所有 .lnk 都在 Start Menu 根目录，递归扫描疑似失效");
}

#[test]
fn start_menu_link_resolves_calculator_target() {
  match launch::find_start_menu_link("Calculator") {
    Some(link) => {
      assert!(link
        .path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("lnk")));
      // IShellLinkW + IPersistFile 目标读取成功且指向可执行文件。
      assert!(
        link.target_exe
          .as_deref()
          .is_some_and(|t| t.to_lowercase().ends_with(".exe")),
        "IShellLinkW 目标读取失败: {:?}",
        link.target_exe
      );
    }
    None => {
      // 正常 Windows 至少带 Calculator.lnk；整体无链接的极简环境才允许缺席。
      assert!(
        launch::scan_start_menu_links().is_empty(),
        "Start Menu 存在 .lnk 但未匹配到 Calculator 显示名"
      );
    }
  }
}
