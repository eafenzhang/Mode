//! Task 5 `perform` 集成测试。
//!
//! brief Step 1 三用例按原文落地（`keysym_chord_parses` / `scroll_amount_clamped_to_100_pages`
//! / `click_on_fixture_reports_dispatched`），其外补：真实输入可观测（光标位置、EDIT 内容
//! 往返、EM_GETSEL 选区、剪贴板还原）、前台零下发、九 kind 解析与参数校验前置。
//!
//! **前台门闩与共享夹具**（controller 对前台校验测试的 flake 指引 → 条件断言不硬断言）：
//! - event 路径有 `foreground_required` 前置校验；同二进制并行用例的夹具互抢前台会互相
//!   打挂——凡会碰前台的用例都经 `spawn_active_fixture`（GATE 串行 + 共享夹具）。
//! - **共享夹具常驻不销毁**：销毁前台窗口会把前台让给外部进程，此后重新夺回前台的
//!   成功率显著下降（串行复现实测 8 连败）——进程级 OnceLock 夹具活到进程退出，
//!   前台始终可回到本进程窗口上。
//! - 激活 = ALT keyup 解前台锁 + AttachThreadInput 借前台线程 + SetForegroundWindow；
//!   仍拿不到前台 → skip（注明），不硬失败。
mod common;

use mode_cua_ax::perform::{self, ElementTarget, Req};
use mode_cua_ax::*;
use std::sync::{MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{FALSE, HWND, LPARAM, POINT, TRUE, WPARAM};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
  keybd_event, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
  GetCursorPos, GetDlgItem, GetForegroundWindow, GetWindowThreadProcessId, SendMessageW,
  SetForegroundWindow,
};

/// 同二进制内「生成夹具 + 前台敏感操作」的互斥门闩（取锁毒化即延续执行，不级联失败）。
static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// 进程级共享夹具：活到测试进程退出（不销毁，见模块注释）。
static SHARED: OnceLock<common::Fixture> = OnceLock::new();

const EM_GETSEL: u32 = 0x00B0; // winuser：返回 LOWORD=选区起、HIWORD=选区止（字符位）

fn hwnd_of(fix: &common::Fixture) -> HWND {
  HWND(fix.hwnd as usize as *mut _)
}

/// 把夹具提到前台（≤1s 重试）：ALT keyup 松开系统前台锁（无对应 down，无副作用）→
/// AttachThreadInput 借前台线程的输入状态 → SetForegroundWindow；单靠后者在前台锁
/// 窗口期 / 外部进程持有前台时会失败（实测）。
fn activate(fix: &common::Fixture) -> bool {
  let hwnd = hwnd_of(fix);
  let deadline = Instant::now() + Duration::from_millis(1000);
  loop {
    // unsafe：keybd_event/AttachThreadInput/SetForegroundWindow 均为 FFI。
    unsafe {
      keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
      let fg = GetForegroundWindow();
      let fg_thread = if fg.is_invalid() {
        0
      } else {
        GetWindowThreadProcessId(fg, None)
      };
      let my = GetCurrentThreadId();
      let attached = fg_thread != 0 && fg_thread != my && AttachThreadInput(fg_thread, my, TRUE).0 != 0;
      let _ = SetForegroundWindow(hwnd);
      if attached {
        let _ = AttachThreadInput(fg_thread, my, FALSE);
      }
    }
    if is_foreground(fix) {
      return true;
    }
    if Instant::now() >= deadline {
      let fg = unsafe { GetForegroundWindow() };
      eprintln!("activate 失败：target={:?} fg={:?}（前台锁/外部占用）", hwnd, fg);
      return false;
    }
    std::thread::sleep(Duration::from_millis(20));
  }
}

fn is_foreground(fix: &common::Fixture) -> bool {
  let fg = unsafe { GetForegroundWindow() };
  fg == hwnd_of(fix)
}

/// 门闩内取共享夹具并激活到前台；拿不到前台 → None（调用方 skip 并注明）。
fn spawn_active_fixture() -> Option<(MutexGuard<'static, ()>, &'static common::Fixture)> {
  let guard = GATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  let fix = SHARED.get_or_init(common::spawn_fixture);
  if !activate(fix) {
    eprintln!("SKIP：本机无法取得前台（前台锁/外部占用），前台敏感断言跳过");
    return None;
  }
  Some((guard, fix))
}

/// observe 里取 edit 元素的 (index, bounds)；bounds 为屏幕坐标（UIA CurrentBoundingRectangle）。
fn edit_of(window_id: u32) -> (u32, Vec<i32>) {
  let r = observe::observe(window_id, 3000).expect("observe");
  let e = r.elements.iter().find(|e| e.kind == "edit").expect("edit");
  (e.index, e.bounds.clone())
}

fn center_of(bounds: &[i32]) -> (i32, i32) {
  (bounds[0] + bounds[2] / 2, bounds[1] + bounds[3] / 2)
}

fn cursor_pos() -> POINT {
  let mut pt = POINT::default();
  let _ = unsafe { GetCursorPos(&mut pt) };
  pt
}

// ————————————————————————— brief Step 1 原文用例 —————————————————————————

#[test]
fn keysym_chord_parses() {
  assert_eq!(
    input::resolve_chord("Return").unwrap(),
    vec![input::KeyStep::Tap(0x0D)]
  );
  // 组合展开：ctrl↓ shift↓ t↓ t↑ shift↑ ctrl↑ = 6 步（修饰键包住主键，抬起顺序镜像）
  assert_eq!(input::resolve_chord("ctrl+shift+t").unwrap().len(), 6);
  assert!(input::resolve_chord("NotAKey").is_err());
}

#[test]
fn scroll_amount_clamped_to_100_pages() {
  // docs 语义是 clamp 不是拒绝：>100 按 100、负数按 0
  assert_eq!(input::wheel_delta_for_pages(101).unwrap(), 12_000); // 120 * 100
  assert_eq!(input::wheel_delta_for_pages(2).unwrap(), 240); // WHEEL_DELTA(120) * pages
  assert_eq!(input::wheel_delta_for_pages(0).unwrap(), 0);
}

#[test]
fn click_on_fixture_reports_dispatched() {
  // 适配（见 task-5-report）：event 前台校验 → 门闩+共享夹具+激活；拿不到前台按
  // controller flake 指引 skip（否则前台违规会把本用例变成环境依赖的硬失败）。
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let r = perform::perform(
    &perform::Req::Click {
      x: 10,
      y: 10,
      button: "left",
      click_count: 1,
      modifiers: "",
    },
    fix.hwnd as u32,
  )
  .unwrap();
  assert_eq!(r, "dispatched");
}

// ————————————————————————— keysym / wheel 纯逻辑 —————————————————————————

#[test]
fn modifier_and_alias_tokens_resolve_to_windows_vks() {
  // controller 裁定 1：表笔误 Shift_L/Shift_L → 第二个按 Shift_R；两侧修饰键齐全。
  let cases = [
    ("Control_L", 0xA2u16),
    ("Control_R", 0xA3),
    ("Shift_L", 0xA0),
    ("Shift_R", 0xA1),
    ("Alt_L", 0xA4),
    ("Alt_R", 0xA5),
    ("super", 0x5B), // Windows 键
    ("ctrl", 0x11),  // VK_CONTROL（brief 指定）
    ("alt", 0x12),   // VK_MENU
    ("shift", 0x10), // VK_SHIFT（brief 测试 ctrl+shift+t 用到，强制入表）
  ];
  for (token, vk) in cases {
    assert_eq!(
      input::resolve_chord(token).unwrap(),
      vec![input::KeyStep::Tap(vk)],
      "token {token}"
    );
  }
}

#[test]
fn named_keys_and_single_chars_resolve() {
  assert_eq!(
    input::resolve_chord("Page_Down").unwrap(),
    vec![input::KeyStep::Tap(0x22)]
  );
  assert_eq!(
    input::resolve_chord("F12").unwrap(),
    vec![input::KeyStep::Tap(0x7B)]
  );
  // 组合里出现未知 token 同样整体拒绝。
  assert!(input::resolve_chord("ctrl+NotAKey").is_err());
  // 单字符走 VkKeyScanW；空串/裸加号为非法 chord。
  assert!(input::resolve_chord("").is_err());
  assert!(input::resolve_chord("ctrl++t").is_err());
}

#[test]
fn wheel_delta_negative_clamps_to_zero() {
  assert_eq!(input::wheel_delta_for_pages(-3).unwrap(), 0);
}

// ————————————————————————— 真实输入可观测（SendInput 落到夹具） —————————————————————————

#[test]
fn click_moves_cursor_to_coordinates() {
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let r = perform::perform(
    &Req::Click {
      x: 240,
      y: 180,
      button: "left",
      click_count: 1,
      modifiers: "ctrl",
    },
    fix.hwnd as u32,
  )
  .unwrap();
  assert_eq!(r, "dispatched");
  // SendInput 由系统输入线程消化，留一个短窗再读光标。
  std::thread::sleep(Duration::from_millis(50));
  let pt = cursor_pos();
  assert_eq!((pt.x, pt.y), (240, 180), "绝对坐标移动未落屏");
}

#[test]
fn key_chord_dispatches_with_repeat_and_hold() {
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let r = perform::perform(
    &Req::Key {
      chord: "ctrl+shift+t",
      repeat: 2,
      hold_ms: 30,
    },
    fix.hwnd as u32,
  )
  .unwrap();
  assert_eq!(r, "dispatched");
}

#[test]
fn type_text_lands_in_fixture_edit() {
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let hwnd = fix.hwnd as u32;
  let (_idx, bounds) = edit_of(hwnd);
  let (cx, cy) = center_of(&bounds);
  // 先点击聚焦（UIA bounds 即屏幕坐标），再键入。
  assert_eq!(
    perform::perform(
      &Req::Click {
        x: cx,
        y: cy,
        button: "left",
        click_count: 1,
        modifiers: "",
      },
      hwnd
    )
    .unwrap(),
    "dispatched"
  );
  assert_eq!(
    perform::perform(&Req::TypeText { text: "ZC" }, hwnd).unwrap(),
    "dispatched"
  );
  std::thread::sleep(Duration::from_millis(150));
  let r = observe::observe(hwnd, 3000).expect("re-observe");
  let edit = r.elements.iter().find(|e| e.kind == "edit").expect("edit");
  let value = edit.value.as_deref().unwrap_or_default();
  assert!(value.contains('C'), "EDIT 值未收到键入内容：{value:?}");
}

#[test]
fn scroll_dispatches_and_moves_cursor() {
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let r = perform::perform(
    &Req::Scroll {
      x: 150,
      y: 120,
      direction: "down",
      amount: 2.0,
    },
    fix.hwnd as u32,
  )
  .unwrap();
  assert_eq!(r, "dispatched");
  std::thread::sleep(Duration::from_millis(50));
  let pt = cursor_pos();
  assert_eq!((pt.x, pt.y), (150, 120), "滚动前的定位移动未落屏");
}

#[test]
fn click_drag_moves_cursor_to_end_point() {
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let r = perform::perform(
    &Req::ClickDrag {
      from_x: 60,
      from_y: 60,
      to_x: 220,
      to_y: 160,
      modifiers: "",
    },
    fix.hwnd as u32,
  )
  .unwrap();
  assert_eq!(r, "dispatched");
  std::thread::sleep(Duration::from_millis(50));
  let pt = cursor_pos();
  assert_eq!((pt.x, pt.y), (220, 160), "拖拽终点未落屏");
}

#[test]
fn set_value_point_runs_event_fallback() {
  // 「x,y」焦点变体：无 UIA 元素可依 → 点击聚焦 + Ctrl+A + 键入（事件兜底全链）。
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let hwnd = fix.hwnd as u32;
  let (_idx, bounds) = edit_of(hwnd);
  let (cx, cy) = center_of(&bounds);
  assert_eq!(
    perform::perform(
      &Req::SetValue {
        target: ElementTarget::Point { x: cx, y: cy },
        value: "PT",
      },
      hwnd
    )
    .unwrap(),
    "dispatched"
  );
  std::thread::sleep(Duration::from_millis(200));
  let r = observe::observe(hwnd, 3000).expect("re-observe");
  let edit = r.elements.iter().find(|e| e.kind == "edit").expect("edit");
  assert_eq!(edit.value.as_deref(), Some("PT"), "事件兜底未替换值");
  // 空串走「全选 + Delete」分支（只选不键入不会清空）。
  assert_eq!(
    perform::perform(
      &Req::SetValue {
        target: ElementTarget::Point { x: cx, y: cy },
        value: "",
      },
      hwnd
    )
    .unwrap(),
    "dispatched"
  );
  std::thread::sleep(Duration::from_millis(200));
  let r2 = observe::observe(hwnd, 3000).expect("re-observe");
  let edit2 = r2.elements.iter().find(|e| e.kind == "edit").expect("edit");
  assert_eq!(
    edit2.value.as_deref(),
    Some(""),
    "空串分支未清空值：{:?}",
    edit2.value
  );
}

// ————————————————————————— 前台校验（零下发） —————————————————————————

#[test]
fn background_window_gets_foreground_required() {
  let Some((_gate, fix_a)) = spawn_active_fixture() else {
    return;
  };
  // 第二个窗口：SW_SHOW 激活会抢走前台 → 重新夺回共享夹具后再制造「B 后台」局面。
  let fix_b = common::spawn_fixture();
  if !activate(fix_a) {
    eprintln!("SKIP：重新激活共享夹具失败，前台零下发断言跳过");
    return;
  }
  let err = perform::perform(
    &Req::Click {
      x: 10,
      y: 10,
      button: "left",
      click_count: 1,
      modifiers: "",
    },
    fix_b.hwnd as u32,
  )
  .unwrap_err();
  assert_eq!(err.code, "foreground_required");
  // 零下发旁证：若点击真的下发到 B（标题栏区域会激活 B），前台应已易主。
  assert!(
    is_foreground(fix_a),
    "前台仍是共享夹具：B 未收到注入（foreground_required 前零下发）"
  );
  // B 是本用例专属夹具，用完显式销毁（共享夹具 A 活到进程退出）。
  fix_b.destroy();
}

// ————————————————————————— UIA 路径（无前台依赖，走 STA 队列） —————————————————————————

#[test]
fn set_value_roundtrips_via_value_pattern() {
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let hwnd = fix.hwnd as u32;
  let (idx, _bounds) = edit_of(hwnd);
  let r = perform::perform(
    &Req::SetValue {
      target: ElementTarget::Index(idx),
      value: "replaced",
    },
    hwnd,
  )
  .unwrap();
  assert_eq!(r, "dispatched");
  std::thread::sleep(Duration::from_millis(50));
  let r2 = observe::observe(hwnd, 3000).expect("re-observe");
  let edit = r2.elements.iter().find(|e| e.kind == "edit").expect("edit");
  assert_eq!(edit.value.as_deref(), Some("replaced"));
}

#[test]
fn select_text_sets_edit_selection() {
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let hwnd = fix.hwnd as u32;
  // 单行 EDIT 实测无 TextPattern（not_selectable 是真实映射）；TextPattern 宿主是夹具的
  // RichEdit20W（按初值唯一定位；ValuePattern 读值带行尾 \r，实测 'rich text\r'）。
  let r = observe::observe(hwnd, 3000).expect("observe");
  let rich = r
    .elements
    .iter()
    .find(|e| e.value.as_deref().is_some_and(|v| v.starts_with("rich text")))
    .expect("RichEdit 元素");
  let idx = rich.index;
  let out = perform::perform(
    &Req::SelectText {
      element_index: idx,
      start: 1,
      length: 2,
    },
    hwnd,
  )
  .unwrap();
  assert_eq!(out, "dispatched");
  std::thread::sleep(Duration::from_millis(50));
  // 经 RichEdit 子窗口（ID=3）EM_GETSEL 观察真实选区。
  let target = unsafe { GetDlgItem(hwnd_of(fix), 3) }.expect("GetDlgItem(RichEdit)");
  let sel = unsafe { SendMessageW(target, EM_GETSEL, WPARAM(0), LPARAM(0)) };
  let raw = sel.0 as usize as u32;
  assert_eq!(
    (raw & 0xFFFF, (raw >> 16) & 0xFFFF),
    (1, 3),
    "EM_GETSEL 选区"
  );
}

#[test]
fn select_text_on_plain_edit_is_not_selectable() {
  // 契约：元素不支持 TextPattern → not_selectable（单行 EDIT 的真实映射）。
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let hwnd = fix.hwnd as u32;
  let (idx, _bounds) = edit_of(hwnd);
  let err = perform::perform(
    &Req::SelectText {
      element_index: idx,
      start: 0,
      length: 1,
    },
    hwnd,
  )
  .unwrap_err();
  assert_eq!(err.code, "not_selectable");
}

#[test]
fn perform_action_presses_button_and_rejects_unknown() {
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let hwnd = fix.hwnd as u32;
  let r = observe::observe(hwnd, 3000).expect("observe");
  let btn = r
    .elements
    .iter()
    .find(|e| e.kind == "button")
    .expect("button");
  let idx = btn.index;
  assert!(
    btn.actions.contains(&"press".to_string()),
    "夹具按钮应有 press"
  );
  assert_eq!(
    perform::perform(
      &Req::Action {
        element_index: idx,
        action: "press",
      },
      hwnd
    )
    .unwrap(),
    "dispatched"
  );
  // 未列出的动作 → action_unavailable（不猜 Legacy 默认动作）。
  let err = perform::perform(
    &Req::Action {
      element_index: idx,
      action: "frobnicate",
    },
    hwnd,
  )
  .unwrap_err();
  assert_eq!(err.code, "action_unavailable");
}

// ————————————————————————— paste：剪贴板保存/写入/还原 + 文本落地 —————————————————————————

#[test]
fn paste_restores_clipboard_and_lands_text() {
  let Some((_gate, fix)) = spawn_active_fixture() else {
    return;
  };
  let hwnd = fix.hwnd as u32;
  clipboard::write_text("USER_CLIP").expect("预置用户剪贴板");
  let (_idx, bounds) = edit_of(hwnd);
  let (cx, cy) = center_of(&bounds);
  assert_eq!(
    perform::perform(
      &Req::Click {
        x: cx,
        y: cy,
        button: "left",
        click_count: 1,
        modifiers: "",
      },
      hwnd
    )
    .unwrap(),
    "dispatched"
  );
  let r = perform::perform(&Req::Paste { text: "PASTED-9" }, hwnd).unwrap();
  assert_eq!(r, "dispatched");
  std::thread::sleep(Duration::from_millis(250));
  // 粘贴内容进入 EDIT。
  let r2 = observe::observe(hwnd, 3000).expect("re-observe");
  let edit = r2.elements.iter().find(|e| e.kind == "edit").expect("edit");
  assert!(
    edit.value.as_deref().unwrap_or_default().contains("PASTED-9"),
    "EDIT 未收到粘贴：{:?}",
    edit.value
  );
  // 剪贴板已还原为用户内容。
  assert_eq!(
    clipboard::read_text().expect("读剪贴板").as_deref(),
    Some("USER_CLIP"),
    "paste 结束后剪贴板未还原"
  );
}

// ————————————————————————— 解析与校验（napi payload 面） —————————————————————————

#[test]
fn parse_req_covers_all_nine_kinds() {
  let cases: &[(&str, &str)] = &[
    ("click", r#"{"x":10,"y":10,"button":"left","clickCount":1,"modifiers":""}"#),
    ("click_drag", r#"{"fromX":1,"fromY":2,"toX":30,"toY":40,"modifiers":"ctrl"}"#),
    ("scroll", r#"{"x":1,"y":2,"direction":"down","amount":3}"#),
    ("key", r#"{"chord":"ctrl+c","repeat":2,"holdMs":50}"#),
    ("type_text", r#"{"text":"hi"}"#),
    ("set_value", r#"{"elementIndex":4,"value":"v"}"#),
    ("set_value", r#"{"x":5,"y":6,"value":"v"}"#), // 「x,y」焦点变体
    ("select_text", r#"{"elementIndex":0,"start":1,"length":2}"#),
    ("action", r#"{"elementIndex":0,"action":"press"}"#),
    ("paste", r#"{"text":"t"}"#),
  ];
  for (kind, payload) in cases {
    let v: serde_json::Value = serde_json::from_str(payload).expect("payload json");
    assert!(perform::parse_req(kind, &v).is_ok(), "{kind} {payload}");
  }
  // 未知 kind → invalid_request。
  let empty: serde_json::Value = serde_json::from_str("{}").expect("json");
  assert_eq!(
    perform::parse_req("bogus", &empty).unwrap_err().code,
    "invalid_request"
  );
}

#[test]
fn validation_precedes_foreground_and_injection() {
  // 参数非法先于前台校验（window_id=0 必非前台——若先查前台会错报 foreground_required）。
  let bad_button = Req::Click {
    x: 0,
    y: 0,
    button: "zz",
    click_count: 1,
    modifiers: "",
  };
  assert_eq!(
    perform::perform(&bad_button, 0).unwrap_err().code,
    "invalid_request"
  );
  let neg = Req::SelectText {
    element_index: 0,
    start: -1,
    length: 1,
  };
  assert_eq!(
    perform::perform(&neg, 0).unwrap_err().code,
    "invalid_request"
  );
}

