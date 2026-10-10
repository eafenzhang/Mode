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

/// 子进程收割守卫（收养过滤用例的探针 cmd）：kill + wait 双收，panic 路径不留进程。
struct ReapChild(Option<std::process::Child>);

impl Drop for ReapChild {
  fn drop(&mut self) {
    if let Some(mut child) = self.0.take() {
      let _ = child.kill();
      let _ = child.wait();
    }
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
  // 修复原因：窗口创建是异步的，进程先起、窗口后到；Release CI（windows-latest）实机
  // 失败过一次——launch resolved pid 后立即单发枚举为空（8 passed; 1 failed）。断言改
  // 有界轮询（5s × 100ms），断的是「最终确有窗口」这个语义，不再赌枚举时机。
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  let wins = loop {
    let wins = mode_cua_ax::apps::list_windows(Some(app.pid)).unwrap();
    if !wins.is_empty() || std::time::Instant::now() >= deadline {
      break wins;
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
  };
  assert!(
    !wins.is_empty(),
    "回填 pid {pid} 下没有窗口（5s 内未出现）",
    pid = app.pid
  );
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

#[test]
fn adoption_filters_pre_snapshot_processes() {
  // I1 收养闸（纯函数直驱）：快照 = 本测试进程已存在、探针进程尚未启动的时刻。
  let pre = launch::snapshot().expect("snapshot");
  // 空候选 → None（backfill 据此降级：lnk 无 spawn 时 pid=0，绝不认领）。
  assert_eq!(launch::adopt_candidate(&pre, &[], None), None);
  // 老进程（本测试二进制，创建于快照前）：即便无 exe 过滤也一律出局。
  let old_pid = std::process::id();
  assert_eq!(launch::adopt_candidate(&pre, &[old_pid], None), None);
  // 快照之后才创建的进程（真实拉起 cmd 探针，Drop 守卫收割）唯一幸存；
  // exe 名是第二道闸：匹配放行、不匹配出局。
  let child = ReapChild(Some(
    std::process::Command::new("cmd")
      .args(["/c", "ping", "-n", "60", "127.0.0.1"])
      .stdout(std::process::Stdio::null())
      .stderr(std::process::Stdio::null())
      .spawn()
      .expect("spawn cmd probe"),
  ));
  let new_pid = child.0.as_ref().expect("probe child").id();
  assert_eq!(launch::adopt_candidate(&pre, &[old_pid, new_pid], None), Some(new_pid));
  assert_eq!(
    launch::adopt_candidate(&pre, &[old_pid, new_pid], Some("cmd.exe")),
    Some(new_pid)
  );
  assert_eq!(
    launch::adopt_candidate(&pre, &[old_pid, new_pid], Some("wrong.exe")),
    None
  );
}
