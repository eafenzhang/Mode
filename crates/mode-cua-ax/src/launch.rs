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
//!   「启动前快照之外的新窗」收养，候选必须过 [`adopt_candidate`] 两道闸
//!   （进程创建时刻晚于快照 + exe 名匹配）——滤光不认领，宁可 pid=0 也不把
//!   期望应用名钉到无关进程上；超时仍返回进程已启动的 AppInfo。
use std::collections::HashSet;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, FILETIME, FALSE, HWND, RPC_E_CHANGED_MODE};
use windows::Win32::Storage::FileSystem::SearchPathW;
use windows::Win32::System::Com::{
  CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
  IPersistFile, STGM_READ,
};
use windows::Win32::System::Threading::{
  CreateProcessW, GetProcessTimes, OpenProcess, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
  PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
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
    let pre = snapshot();
    let spawn_pid = spawn_path(&path)?;
    // 收养带 exe 匹配（转发型启动：stub 拉起真窗口时窗口 pid ≠ spawn pid）；
    // 候选仍过创建时间闸（I1），自家 spawn 窗口则走优先级 1 不经收养。
    let pid = backfill_pid(
      pre.as_ref(),
      Some(spawn_pid),
      &Adopt::NewWindow {
        exe: Some(exe_name.clone()),
      },
    );
    return Ok(finish(pid, Some(exe_name), None));
  }
  // 第 3 段：Start Menu 两个根的 .lnk 显示名匹配 → ShellExecuteW（无 pid，新窗收养回填）。
  let Some(link) = find_start_menu_link(name) else {
    return Err(AxError::new(
      "launch_failed",
      format!("无法解析应用「{name}」：SearchPathW 未命中，Start Menu 无匹配 .lnk"),
    ));
  };
  let pre = snapshot();
  shell_execute_lnk(&link.path)?;
  // 目标可读 → 附加 exe 匹配；读不出 → 任意新窗（仍过 I1 创建时间闸，滤光 → pid=0，
  // 绝不按 z 序认领首个新窗——旧进程的 toast/弹窗会被闸掉）。
  let adopt = Adopt::NewWindow {
    exe: link.target_exe.clone(),
  };
  let pid = backfill_pid(pre.as_ref(), None, &adopt);
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
  let pre = snapshot();
  let spawn_pid = unsafe {
    mgr.ActivateApplication(PCWSTR(wide.as_ptr()), PCWSTR::null(), AO_NONE)
  }
  .map_err(|e| AxError::new("launch_failed", format!("ActivateApplication({aumid}) 失败: {e}")))?;
  let pid = backfill_pid(pre.as_ref(), Some(spawn_pid), &Adopt::SpawnOnly);
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

/// 启动前快照：可见窗口 pid 集 + 快照起点时刻（`at`，SystemTime 与进程创建时间同钟域，
/// 在枚举之前取——真目标 spawn 于快照之后，创建时间必晚于它）。
/// 枚举失败返回 None（M4：没有可信基线时收养整体降级 SpawnOnly，绝不拿空集当“全新”）。
pub struct PreState {
  pids: HashSet<u32>,
  at: SystemTime,
}

/// 采集启动前快照（新窗收养基准）；枚举失败 → None。
pub fn snapshot() -> Option<PreState> {
  let at = SystemTime::now();
  let hwnds = apps::visible_top_level_windows().ok()?;
  let pids = hwnds
    .iter()
    .map(|h| apps::window_pid(*h))
    .filter(|p| *p != 0)
    .collect();
  Some(PreState { pids, at })
}

/// FILETIME（1601 起点、每 100ns 一格）→ SystemTime；未到 Unix 纪元或换算溢出 → None。
fn filetime_to_systemtime(ft: FILETIME) -> Option<SystemTime> {
  // 1601→1970 共 11_644_473_600 秒 × 每秒 10^7 个 100ns 格（不是 10^8——初版乘错导致
  // checked_sub 恒失败、过滤恒 false，被 adoption 用例当场抓住）。
  const FILETIME_UNIX_EPOCH_100NS: u64 = 11_644_473_600 * 10_000_000;
  let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
  let since_unix = ticks.checked_sub(FILETIME_UNIX_EPOCH_100NS)?;
  SystemTime::UNIX_EPOCH.checked_add(Duration::from_micros(since_unix / 10))
}

/// 该 pid 的进程创建时刻是否晚于快照（I1 收养闸）。打开/读取失败一律视为不可信
/// → false（排除）：方向上宁可漏认（最终 pid=0）也不误认旧进程的晚出窗口。
fn process_created_after(pid: u32, after: SystemTime) -> bool {
  let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
    return false;
  };
  // 读创建时间放闭包：单一出口关闭句柄，任何早退都不泄。
  let created = (|| -> Option<SystemTime> {
    let mut create = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetProcessTimes(handle, &mut create, &mut exit, &mut kernel, &mut user) }.ok()?;
    filetime_to_systemtime(create)
  })();
  let _ = unsafe { CloseHandle(handle) };
  created.is_some_and(|c| c > after)
}

/// I1 收养过滤（纯函数面，测试直驱）：候选 = 启动前快照之外的窗口 pid，再过两道可信度闸——
/// ① 进程创建时刻晚于快照（toast/既有进程的新窗口出局）；② `want_exe` 给定时 exe 名匹配。
/// 全滤光 → None：调用方降级（spawn 已知回 spawn，lnk 无 spawn 则 pid=0），绝不按 z 序
/// 认领首个新窗——把期望应用名钉到无关进程上是对模型的自洽谎言。
pub fn adopt_candidate(pre: &PreState, fresh: &[u32], want_exe: Option<&str>) -> Option<u32> {
  let want = want_exe.map(|w| w.to_lowercase());
  for &pid in fresh {
    if !process_created_after(pid, pre.at) {
      continue;
    }
    if let Some(want) = &want {
      match apps::exe_name_for_pid(pid) {
        Some(exe) if exe.to_lowercase() == *want => {}
        _ => continue,
      }
    }
    return Some(pid);
  }
  None
}

/// pid 回填策略（按启动路径选取）。
enum Adopt {
  /// 只等 spawn 自己的窗口出现（AUMID：返回的 pid 即目标进程，不收养旁窗）。
  SpawnOnly,
  /// 收养新窗：`exe` 为 Some 时附加 exe 名匹配（SearchPathW / lnk 目标可读），
  /// None 为任意通过创建时间闸的新窗（lnk 目标读不出的退化口径）。
  NewWindow { exe: Option<String> },
}

/// spawn 后 EnumWindows 轮询 ≤3s 等新窗回填 pid（brief Step 3）：
/// 1) spawn 自家窗口出现 → 该 pid；2) `pre` 可信（M4）时按 [`Adopt`] 收养——候选过
///    [`adopt_candidate`] 创建时间/exe 双闸，滤光不认领；3) 超时 → spawn pid
///    （无 spawn 时为 0：进程已由 shell 成功拉起，但窗口未在时限内出现或不可认领）。
///
/// M2：50ms 轮询 + 单次枚举耗时会让实际截止越过字面 3s 约一拍（~50ms + 枚举时间），
/// 工程可接受——精确到拍而非精确到毫秒，语义仍是「≤3s 量级的有界等待」。
fn backfill_pid(pre: Option<&PreState>, spawn: Option<u32>, adopt: &Adopt) -> u32 {
  let deadline = Instant::now() + BACKFILL_TIMEOUT;
  loop {
    // 单拍枚举失败按「本拍无候选」处理（下一拍重试）；收养基线是 pre，不受单拍影响。
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
      // pre=None（M4 快照失效）时不收集候选 → 收养自然降级为 SpawnOnly。
      if pre.is_some_and(|p| !p.pids.contains(&pid)) {
        fresh.push(pid);
      }
    }
    if let (Some(pre), Adopt::NewWindow { exe }) = (pre, adopt) {
      if let Some(pid) = adopt_candidate(pre, &fresh, exe.as_deref()) {
        return pid;
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
    // M1：不跟随 symlink/junction（Windows 上二者同为 name-surrogate 再解析点，
    // Rust 的 is_symlink 对 mount point 同样为真）——扫描被约束在 Start Menu 根内，
    // 不被指向根外的链接带出去（深度上限是第二道兜底）。
    if ft.is_symlink() {
      continue;
    }
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
