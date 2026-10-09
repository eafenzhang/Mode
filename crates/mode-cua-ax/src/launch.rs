//! 应用启动（Task 6 的纯逻辑面）：brief 的三段解析顺序 + 启动后窗口 pid 回填。
//!
//! 解析顺序（brief Produces 逐条）：
//! 1. `Target::BundleId` 含 `.` → `IApplicationActivationManager::ActivateApplication(AUMID)`；
//!    激活失败按 spec「bundle_id 优先 AUMID，回退可执行文件路径」把值当路径再走第 2 段；
//! 2. `SearchPathW(name.exe)` 命中 → `CreateProcessW` 拉起（spawn 即拿到 pid）；
//! 3. 再否则扫 Start Menu 两个根的 `.lnk`（显示名匹配）→ `ShellExecuteW`；
//!
//! 全失败 → `launch_failed`；参数非法（空名/空 bundleId）→ `invalid_request`。
//!
//! pid 回填：spawn 后 `EnumWindows` 轮询 ≤3s 等新窗——
//! - AUMID / SearchPathW 路径 spawn 已给 pid，自家窗口出现即确认；
//! - 转发型启动（stub 进程拉起真窗口、lnk 的 ShellExecuteW 本身无 pid）按
//!   「启动前快照之外的新窗 + exe 名匹配」收养；超时仍返回进程已启动的 AppInfo
//!   （lnk 收养不到时 pid=0，进程本身已成功启动）。
use std::collections::HashSet;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, FALSE, HWND, RPC_E_CHANGED_MODE};
use windows::Win32::Storage::FileSystem::SearchPathW;
use windows::Win32::System::Com::{
  CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
  IPersistFile, STGM_READ,
};
use windows::Win32::System::Threading::{
  CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Shell::{
  ApplicationActivationManager, IApplicationActivationManager, IShellLinkW, AO_NONE, ShellExecuteW,
  ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::apps;
use crate::error::{AxError, AxResult};
use crate::{AppInfoNapi, LaunchRequestNapi};

/// pid 回填时限与轮询间隔（brief：≤3s；50ms 一拍，窗口出现即返回）。
const BACKFILL_TIMEOUT: Duration = Duration::from_secs(3);
const BACKFILL_POLL: Duration = Duration::from_millis(50);

/// Start Menu 扫描深度上限：正常树深 ≤5，junction/异常树不至于扫飞。
const SCAN_MAX_DEPTH: usize = 10;

/// 启动目标（brief 测试面：`Target::Name`；napi 请求面由 [`target_from`] 归一化）。
/// Debug：测试面 `.unwrap_err()` 断言错误码需要 Ok 类型 Debug（同 AppInfoNapi 口径）。
#[derive(Debug)]
pub enum Target {
  /// 按可执行/显示名解析（SearchPathW → Start Menu .lnk）。
  Name(String),
  /// bundleId：含 `.` 视为 AUMID 走激活；否则按名字解析（SDK 把裸字符串放 bundleId）。
  BundleId(String),
}

/// napi `launch_app` 入口：请求归一化后走 [`launch`]。
pub fn launch_request(req: &LaunchRequestNapi) -> AxResult<AppInfoNapi> {
  launch(&target_from(req.name.clone(), req.bundle_id.clone())?)
}

/// 请求面归一化（brief：「二选一，SDK 保证；裸字符串按 bundleId 传入」）：
/// 点状 bundleId → `Target::BundleId`（解析顺序首位，即便同时带 name 也以它优先）；
/// 否则有 name 用 name；只有 bundleId 时把它当名字解析（非点状的裸字符串）；
/// 两者皆空 → `invalid_request`。
pub fn target_from(name: Option<String>, bundle_id: Option<String>) -> AxResult<Target> {
  let bundle = bundle_id
    .map(|s| s.trim().to_owned())
    .filter(|s| !s.is_empty());
  let name = name.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
  match (name, bundle) {
    (_, Some(b)) if b.contains('.') => Ok(Target::BundleId(b)),
    (Some(n), _) => Ok(Target::Name(n)),
    (None, Some(b)) => Ok(Target::Name(b)),
    (None, None) => Err(AxError::new(
      "invalid_request",
      "launch_app 需要 name 或 bundleId 之一（两者均为空）",
    )),
  }
}

/// 按三段解析顺序启动应用（brief 测试面入口）。
pub fn launch(target: &Target) -> AxResult<AppInfoNapi> {
  match target {
    Target::Name(name) => launch_name(name),
    Target::BundleId(id) => {
      let id = id.trim();
      if id.is_empty() {
        return Err(AxError::new("invalid_request", "bundleId 为空"));
      }
      if id.contains('.') {
        match activate_aumid(id) {
          Ok(info) => Ok(info),
          // spec「bundle_id 优先 AUMID，回退可执行文件路径」：AUMID 激活失败且错误
          // 属启动语义（而非 COM 设施故障）时，把值当路径/名字再走第 2、3 段。
          Err(e) if e.code == "launch_failed" => launch_name(id).map_err(|fallback| {
            AxError::new(
              "launch_failed",
              format!(
                "AUMID 激活失败（{}）；可执行路径回退亦失败（{}）",
                e.message, fallback.message
              ),
            )
          }),
          Err(e) => Err(e),
        }
      } else {
        // 非点状 bundleId = SDK 传入的裸字符串，按名字解析链处理（brief 的「否则」分支）。
        launch_name(id)
      }
    }
  }
}

/// 名字解析：SearchPathW → Start Menu .lnk 显示名 → launch_failed。
fn launch_name(name: &str) -> AxResult<AppInfoNapi> {
  let name = name.trim();
  if name.is_empty() {
    return Err(AxError::new("invalid_request", "launch name 为空"));
  }
  // 输入自带扩展名时不重复补 .exe（大小写不敏感）。
  let exe = if name.to_ascii_lowercase().ends_with(".exe") {
    name.to_owned()
  } else {
    format!("{name}.exe")
  };
  if let Some(path) = search_path_w(&exe) {
    let exe_name = path
      .file_name()
      .map(|n| n.to_string_lossy().into_owned())
      .unwrap_or_else(|| exe.clone());
    let pre = visible_pids();
    let spawn_pid = spawn_path(&path)?;
    // 匹配 exe 名的新窗收养：转发型启动（stub 拉起真窗口）时窗口 pid ≠ spawn pid。
    let pid = backfill_pid(&pre, Some(spawn_pid), &Adopt::MatchExe(exe_name.clone()));
    return Ok(finish(pid, Some(exe_name), None));
  }
  // 第 3 段：Start Menu 两个根的 .lnk 显示名匹配 → ShellExecuteW（无 pid，新窗收养回填）。
  let Some(link) = find_start_menu_link(name) else {
    return Err(AxError::new(
      "launch_failed",
      format!("无法解析应用「{name}」：SearchPathW 未命中，Start Menu 无匹配 .lnk"),
    ));
  };
  let pre = visible_pids();
  shell_execute_lnk(&link.path)?;
  let adopt = match &link.target_exe {
    Some(exe) => Adopt::MatchExe(exe.clone()),
    None => Adopt::AnyNew, // 目标读不出时退化为 brief 的「等新窗」字面口径
  };
  let pid = backfill_pid(&pre, None, &adopt);
  Ok(finish(pid, Some(link.display_name), None))
}

/// 第 1 段：IApplicationActivationManager::ActivateApplication(AUMID)。
/// 返回的 pid 即目标进程，窗口出现即确认（只等自家窗口，不收养旁窗）。
fn activate_aumid(aumid: &str) -> AxResult<AppInfoNapi> {
  let _com = com_init()?;
  let wide: Vec<u16> = aumid.encode_utf16().chain(std::iter::once(0)).collect();
  let mgr: IApplicationActivationManager = unsafe {
    CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_INPROC_SERVER)
  }
  .map_err(|e| {
    AxError::new(
      "launch_failed",
      format!("创建 IApplicationActivationManager 失败: {e}"),
    )
  })?;
  // 快照先于激活：激活期间冒出的既有窗口不算「新窗」。
  let pre = visible_pids();
  let spawn_pid = unsafe {
    mgr.ActivateApplication(PCWSTR(wide.as_ptr()), PCWSTR::null(), AO_NONE)
  }
  .map_err(|e| AxError::new("launch_failed", format!("ActivateApplication({aumid}) 失败: {e}")))?;
  let pid = backfill_pid(&pre, Some(spawn_pid), &Adopt::SpawnOnly);
  Ok(finish(
    pid,
    apps::exe_name_for_pid(pid),
    Some(aumid.to_string()),
  ))
}

/// SearchPathW（lpPath=NULL：应用目录→当前目录→System32→Windows→PATH）；
/// 命中返回完整路径，0（未命中）→ None。
fn search_path_w(exe: &str) -> Option<PathBuf> {
  let wide: Vec<u16> = exe.encode_utf16().chain(std::iter::once(0)).collect();
  let mut buf = vec![0u16; 32768];
  let n = unsafe {
    SearchPathW(
      PCWSTR::null(),
      PCWSTR(wide.as_ptr()),
      PCWSTR::null(),
      Some(&mut buf),
      None,
    )
  };
  if n == 0 || n as usize >= buf.len() {
    return None;
  }
  Some(PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])))
}

/// CreateProcessW 拉起解析出的可执行文件，返回 pid；失败 → launch_failed。
/// hProcess/hThread 用完即关：拿到 pid 即可，进程生命周期归窗口回填与调用方。
fn spawn_path(path: &Path) -> AxResult<u32> {
  let app: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
  // 命令行 = 带引号的完整路径（argv[0] 语义；CreateProcessW 可改写缓冲 → Vec 可变）。
  let mut cmd: Vec<u16> = vec![b'"' as u16];
  cmd.extend(path.as_os_str().encode_wide());
  cmd.extend([b'"' as u16, 0]);
  let si = STARTUPINFOW {
    cb: std::mem::size_of::<STARTUPINFOW>() as u32,
    ..Default::default()
  };
  let mut pi = PROCESS_INFORMATION::default();
  unsafe {
    CreateProcessW(
      PCWSTR(app.as_ptr()),
      PWSTR(cmd.as_mut_ptr()),
      None,
      None,
      FALSE,
      PROCESS_CREATION_FLAGS(0),
      None,
      PCWSTR::null(),
      &si,
      &mut pi,
    )
  }
  .map_err(|e| {
    AxError::new(
      "launch_failed",
      format!("CreateProcessW({}) 失败: {e}", path.display()),
    )
  })?;
  unsafe {
    let _ = CloseHandle(pi.hThread);
    let _ = CloseHandle(pi.hProcess);
  }
  Ok(pi.dwProcessId)
}

/// ShellExecuteW 打开 .lnk（shell 层拉起，无 pid 返回——pid 由新窗收养回填）。
fn shell_execute_lnk(path: &Path) -> AxResult<()> {
  let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
  let file: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
  let hinst = unsafe {
    ShellExecuteW(
      HWND::default(),
      PCWSTR(verb.as_ptr()),
      PCWSTR(file.as_ptr()),
      PCWSTR::null(),
      PCWSTR::null(),
      SW_SHOWNORMAL,
    )
  };
  // HINSTANCE 语义：>32 成功，≤32 为错误码。
  if (hinst.0 as isize) <= 32 {
    return Err(AxError::new(
      "launch_failed",
      format!("ShellExecuteW({}) 失败", path.display()),
    ));
  }
  Ok(())
}

/// 启动前的可见窗口 pid 快照（新窗收养基准）；枚举失败按空集降级（回填仍有 spawn 兜底）。
fn visible_pids() -> HashSet<u32> {
  apps::visible_top_level_windows()
    .map(|hwnds| {
      hwnds
        .iter()
        .map(|h| apps::window_pid(*h))
        .filter(|p| *p != 0)
        .collect()
    })
    .unwrap_or_default()
}

/// pid 回填策略（按启动路径选取）。
enum Adopt {
  /// 只等 spawn 自己的窗口出现（AUMID：返回的 pid 即目标进程，不收养旁窗）。
  SpawnOnly,
  /// 收养「快照之外 + exe 名匹配」的新窗（转发型启动：窗口 pid ≠ spawn pid）。
  MatchExe(String),
  /// 收养任意新窗（lnk 目标读不出时的退化口径，brief 的「等新窗回填」字面语义）。
  AnyNew,
}

/// spawn 后 EnumWindows 轮询 ≤3s 等新窗回填 pid（brief Step 3）：
/// 1) spawn 自家窗口出现 → 该 pid；2) 按 [`Adopt`] 收养新窗；
/// 3) 超时 → spawn pid（无 spawn 时为 0：进程已由 shell 成功拉起，仅窗口未在时限内出现）。
fn backfill_pid(pre: &HashSet<u32>, spawn: Option<u32>, adopt: &Adopt) -> u32 {
  let deadline = Instant::now() + BACKFILL_TIMEOUT;
  loop {
    let hwnds = apps::visible_top_level_windows().unwrap_or_default();
    let mut fresh = Vec::new();
    for hwnd in &hwnds {
      let pid = apps::window_pid(*hwnd);
      if pid == 0 {
        continue;
      }
      if spawn == Some(pid) {
        return pid;
      }
      if !pre.contains(&pid) {
        fresh.push(pid);
      }
    }
    match adopt {
      Adopt::SpawnOnly => {}
      Adopt::MatchExe(want) => {
        let want = want.to_lowercase();
        for pid in fresh {
          if apps::exe_name_for_pid(pid).is_some_and(|exe| exe.to_lowercase() == want) {
            return pid;
          }
        }
      }
      Adopt::AnyNew => {
        if let Some(pid) = fresh.first() {
          return *pid;
        }
      }
    }
    if Instant::now() >= deadline {
      break;
    }
    std::thread::sleep(BACKFILL_POLL);
  }
  spawn.unwrap_or(0)
}

/// AppInfo 组装（两条解析路径共用）：`bundle` 为已知 AUMID 时原样回填，
/// 否则按 pid 尽力查 AUMID（经典 Win32 程序通常没有 → null）；active = 前台即该 pid。
fn finish(pid: u32, name: Option<String>, bundle: Option<String>) -> AppInfoNapi {
  AppInfoNapi {
    pid,
    name,
    bundle_id: bundle.or_else(|| apps::aumid_for_pid(pid)),
    active: apps::foreground_pid() == Some(pid),
  }
}

// ————————————————————————— COM 作用域守卫 —————————————————————————

/// COM 作用域守卫：本线程首次初始化成功 → Drop 时 CoUninitialize；
/// `RPC_E_CHANGED_MODE`（线程已以另一模式初始化）不反初始化，仅借用现成的。
struct ComGuard {
  uninit: bool,
}

impl Drop for ComGuard {
  fn drop(&mut self) {
    if self.uninit {
      unsafe { CoUninitialize() };
    }
  }
}

/// 按需初始化 COM（AUMID 激活与 IShellLinkW 读取各走一次；不在调用线程外强制 STA 队列，
/// 与 brief 的同步 launch 语义一致）。设施性失败 → internal（区别于启动语义的 launch_failed）。
fn com_init() -> AxResult<ComGuard> {
  let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
  if hr.is_ok() {
    Ok(ComGuard { uninit: true })
  } else if hr == RPC_E_CHANGED_MODE {
    Ok(ComGuard { uninit: false })
  } else {
    Err(AxError::internal(format!("CoInitializeEx 失败: {hr}")))
  }
}

// ————————————————————————— Start Menu .lnk 解析 —————————————————————————

/// Start Menu 链接条目。`target_exe`：scan 阶段不读（无 COM）；[`find_start_menu_link`]
/// 命中后经 IShellLinkW+IPersistFile 解析，读不出为 None（仍返回条目，启动不依赖它）。
pub struct StartMenuLink {
  /// 显示名（.lnk 主名，即 OS 列出的 Start 菜单名）。
  pub display_name: String,
  /// .lnk 完整路径。
  pub path: PathBuf,
  /// 链接目标的 exe 文件名（如 `calc.exe`）；未读/读失败为 None。
  pub target_exe: Option<String>,
}

/// Start Menu 两个根（brief 点名）：`%APPDATA%\Microsoft\Windows\Start Menu` +
/// `%ProgramData%\Microsoft\Windows\Start Menu`。
fn start_menu_roots() -> Vec<PathBuf> {
  let mut roots = Vec::new();
  if let Some(appdata) = std::env::var_os("APPDATA") {
    roots.push(
      PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu"),
    );
  }
  if let Some(pd) = std::env::var_os("ProgramData") {
    roots.push(
      PathBuf::from(pd)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu"),
    );
  }
  roots
}

/// 递归扫描两个根下全部 `.lnk`（含子目录）。单个子树读目录失败直接跳过——
/// 权限/竞态不该阻断整个扫描。
pub fn scan_start_menu_links() -> Vec<StartMenuLink> {
  let mut out = Vec::new();
  for root in start_menu_roots() {
    collect_links(&root, 0, &mut out);
  }
  out
}

fn collect_links(dir: &Path, depth: usize, out: &mut Vec<StartMenuLink>) {
  if depth > SCAN_MAX_DEPTH {
    return;
  }
  let Ok(rd) = std::fs::read_dir(dir) else {
    return;
  };
  for entry in rd.flatten() {
    let Ok(ft) = entry.file_type() else {
      continue;
    };
    let path = entry.path();
    if ft.is_dir() {
      collect_links(&path, depth + 1, out);
    } else if ft.is_file()
      && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
    {
      if let Some(stem) = path.file_stem() {
        out.push(StartMenuLink {
          display_name: stem.to_string_lossy().into_owned(),
          path,
          target_exe: None,
        });
      }
    }
  }
}

/// 按显示名（大小写不敏感，精确匹配 .lnk 主名）找链接；命中后读一次 IShellLinkW 目标
/// （COM 惰性初始化，只在命中时发生）。
pub fn find_start_menu_link(display: &str) -> Option<StartMenuLink> {
  let want = display.trim().to_lowercase();
  if want.is_empty() {
    return None;
  }
  let mut link = scan_start_menu_links().into_iter().find(|l| {
    let stem = l.display_name.to_lowercase();
    // 输入带不带 .exe 后缀都能命中（显示名本身不含扩展名）。
    stem == want || stem == want.trim_end_matches(".exe")
  })?;
  link.target_exe = lnk_target_exe(&link.path);
  Some(link)
}

/// IShellLinkW::GetPath 读链接目标，返回目标 exe 文件名。任一步失败 → None
/// （收养策略退化为 AnyNew，不阻断启动）。
fn lnk_target_exe(path: &Path) -> Option<String> {
  let _com = com_init().ok()?;
  let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;
  let persist: IPersistFile = link.cast().ok()?;
  let wide: Vec<u16> = path
    .as_os_str()
    .encode_wide()
    .chain(std::iter::once(0))
    .collect();
  unsafe { persist.Load(PCWSTR(wide.as_ptr()), STGM_READ) }.ok()?;
  let mut buf = vec![0u16; 32768];
  unsafe { link.GetPath(&mut buf, std::ptr::null_mut(), 0) }.ok()?;
  let target = String::from_utf16_lossy(&buf);
  let target = target.trim_end_matches('\0');
  if target.is_empty() {
    return None;
  }
  target
    .rsplit(['\\', '/'])
    .next()
    .filter(|n| !n.is_empty())
    .map(|n| n.to_string())
}
