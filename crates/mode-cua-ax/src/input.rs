//! `perform` 输入层：keysym 表、`SendInput` 封装、event 路径前台校验。
//!
//! 语义（plan Task 5 Step 3 + brief 测试钉死的三条 controller 裁定）：
//! - `resolve_chord`：`+` 分隔；单 token → `Tap(vk)`（单字符走 `VkKeyScanW`，无映射 → `Unicode`）；
//!   组合 → 修饰键包住主键、抬起镜像（`ctrl+shift+t` = 6 步：ctrl↓ shift↓ t↓ t↑ shift↑ ctrl↑）；
//!   未知 token / 空 token → `invalid_request`。
//! - `wheel_delta_for_pages`：**clamp** 0..=100 页 × WHEEL_DELTA(120)，不拒绝
//!   （101→12000、2→240、0→0、负→0；brief 裁定 2）。
//! - `ensure_foreground`：`GetForegroundWindow() != hwnd` → `foreground_required`。
//!   这是 event 路径「零下发」的结构保证——调用方必须在构造/投递任何 INPUT 之前先调它。
//!
//! dispatched 三态（brief Step 3）：一次 `SendInput` 收到条数 == 期望 → `dispatched`；
//! 0 → `not_dispatched`；部分/不确定 → `unknown`。跨批次取**合计**（部分批次成功即 unknown，
//! 否则无法区分「全没发」与「发了一半」）。
use crate::error::{AxError, AxResult};
use std::ffi::c_void;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
  SendInput, VkKeyScanW, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
  KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEINPUT, MOUSE_EVENT_FLAGS,
  MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
  MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
  MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, VIRTUAL_KEY, VK_LCONTROL,
  VK_LMENU, VK_LSHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
  GetForegroundWindow, GetSystemMetrics, WHEEL_DELTA, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
  SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// 一次 chord 展开后的按键步。`Tap` = 同批次内的 down+up（原子按下抬起）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStep {
  Down(u16),
  Up(u16),
  Tap(u16),
  Unicode(char),
}

/// `VkKeyScanW` 高字节的修饰标志（1=Shift、2=Ctrl、4=Alt）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ScanMods {
  shift: bool,
  ctrl: bool,
  alt: bool,
}

/// 单 token 的解析结果。
enum TokenKey {
  /// 具名 keysym 或 VkKeyScanW 命中的 VK 码；`mods` 是该字符自带的（如 'A'→Shift）。
  Vk { vk: u16, mods: ScanMods },
  /// 单字符且 `VkKeyScanW` 无映射（如 CJK）→ 只能走 Unicode 注入。
  Unicode(char),
}

/// 具名 keysym 表（brief Step 3 列出的全部 token + 测试强制的 `shift` 别名）。
/// `super`→VK_LWIN、`ctrl`→VK_CONTROL、`alt`→VK_MENU（Windows 语义，brief 指定）。
fn named_keysym(token: &str) -> Option<u16> {
  Some(match token {
    "Return" => 0x0D,
    "Tab" => 0x09,
    "Escape" => 0x1B,
    "BackSpace" => 0x08,
    "Delete" => 0x2E,
    "Up" => 0x26,
    "Down" => 0x28,
    "Left" => 0x25,
    "Right" => 0x27,
    "Home" => 0x24,
    "End" => 0x23,
    "Page_Up" => 0x21,
    "Page_Down" => 0x22,
    "Space" => 0x20,
    "F1" => 0x70,
    "F2" => 0x71,
    "F3" => 0x72,
    "F4" => 0x73,
    "F5" => 0x74,
    "F6" => 0x75,
    "F7" => 0x76,
    "F8" => 0x77,
    "F9" => 0x78,
    "F10" => 0x79,
    "F11" => 0x7A,
    "F12" => 0x7B,
    "Control_L" => 0xA2, // VK_LCONTROL
    "Control_R" => 0xA3, // VK_RCONTROL
    "Shift_L" => 0xA0,   // VK_LSHIFT
    "Shift_R" => 0xA1,   // VK_RSHIFT（裁定 1：表里重复的第二个 Shift_L 按 Shift_R 实现）
    "Alt_L" => 0xA4,     // VK_LMENU
    "Alt_R" => 0xA5,     // VK_RMENU
    "super" => 0x5B,     // VK_LWIN
    "ctrl" => 0x11,      // VK_CONTROL
    "alt" => 0x12,       // VK_MENU
    "shift" => 0x10,     // VK_SHIFT（brief 测试 ctrl+shift+t 用到，强制入表）
    _ => return None,
  })
}

/// 单 token 解析：具名表 → 单字符（VkKeyScanW / Unicode 兜底）→ 非法。
fn resolve_token(token: &str) -> AxResult<TokenKey> {
  if token.is_empty() {
    return Err(AxError::new("invalid_request", "chord 含空 token"));
  }
  if let Some(vk) = named_keysym(token) {
    return Ok(TokenKey::Vk {
      vk,
      mods: ScanMods::default(),
    });
  }
  let mut chars = token.chars();
  if let (Some(c), None) = (chars.next(), chars.next()) {
    // 单字符 → VkKeyScanW（返回 -1 表示当前键盘布局无映射 → Unicode 注入兜底）。
    let scan = u16::try_from(c).ok().map(|unit| unsafe { VkKeyScanW(unit) });
    if let Some(code) = scan {
      if code != -1 {
        let flags = ((code >> 8) & 0xFF) as u8;
        return Ok(TokenKey::Vk {
          vk: (code & 0xFF) as u16,
          mods: ScanMods {
            shift: flags & 1 != 0,
            ctrl: flags & 2 != 0,
            alt: flags & 4 != 0,
          },
        });
      }
    }
    return Ok(TokenKey::Unicode(c));
  }
  Err(AxError::new(
    "invalid_request",
    format!("未知 keysym: {token}"),
  ))
}

/// 把 `VkKeyScanW`/持有键的修饰标志翻译成修饰键 VK 序列（Shift→Ctrl→Alt，稳定顺序）。
fn mod_vks(mods: ScanMods) -> Vec<u16> {
  let mut out = Vec::new();
  if mods.shift {
    out.push(VK_LSHIFT.0);
  }
  if mods.ctrl {
    out.push(VK_LCONTROL.0);
  }
  if mods.alt {
    out.push(VK_LMENU.0);
  }
  out
}

/// 解析 chord → 按键步序列（brief Step 3；三条 controller 裁定的落点）。
///
/// - 单 token：`[修饰(如有)…, Tap(vk), 修饰抬起…]`，无修饰即 `vec![Tap(vk)]`。
/// - 多 token：**最后一个**为主键，其余按住（Down），主键按下抬起后镜像抬起——
///   `ctrl+shift+t` 恰为 6 步（裁定 3）。主键自带修饰（如 `A`）若未被持有键覆盖则补进按住序列。
pub fn resolve_chord(chord: &str) -> AxResult<Vec<KeyStep>> {
  let tokens: Vec<&str> = chord.split('+').map(str::trim).collect();
  if tokens.iter().any(|t| t.is_empty()) {
    return Err(AxError::new("invalid_request", "chord 含空 token"));
  }
  let (main, held) = match (tokens.len(), tokens.split_last()) {
    (0, _) | (_, None) => {
      return Err(AxError::new("invalid_request", "chord 不能为空"));
    }
    (1, Some((only, []))) => (*only, &[][..]),
    (_, Some((last, rest))) => (*last, rest),
  };
  let main_key = resolve_token(main)?;

  if held.is_empty() {
    // 单 token。
    return match main_key {
      TokenKey::Unicode(c) => Ok(vec![KeyStep::Unicode(c)]),
      TokenKey::Vk { vk, mods } => {
        let presses = mod_vks(mods);
        let mut steps = Vec::with_capacity(presses.len() * 2 + 1);
        for m in &presses {
          steps.push(KeyStep::Down(*m));
        }
        steps.push(KeyStep::Tap(vk));
        for m in presses.iter().rev() {
          steps.push(KeyStep::Up(*m));
        }
        Ok(steps)
      }
    };
  }

  // 组合：按住序列（去重、保序）= 指定持有键 + 主键自带修饰。
  let mut down_order: Vec<u16> = Vec::new();
  for token in held {
    match resolve_token(token)? {
      TokenKey::Vk { vk, .. } => {
        if !down_order.contains(&vk) {
          down_order.push(vk);
        }
      }
      TokenKey::Unicode(_) => {
        return Err(AxError::new(
          "invalid_request",
          format!("组合键中不可按住的 keysym: {token}"),
        ));
      }
    }
  }
  if let TokenKey::Vk { mods, .. } = &main_key {
    for m in mod_vks(*mods) {
      if !down_order.contains(&m) {
        down_order.push(m);
      }
    }
  }
  let mut steps = Vec::with_capacity(down_order.len() * 2 + 2);
  for vk in &down_order {
    steps.push(KeyStep::Down(*vk));
  }
  match main_key {
    // 主键按下/抬起显式两步（与持有键同构，保证 6 步形状）。
    TokenKey::Vk { vk, .. } => {
      steps.push(KeyStep::Down(vk));
      steps.push(KeyStep::Up(vk));
    }
    TokenKey::Unicode(c) => steps.push(KeyStep::Unicode(c)),
  }
  for vk in down_order.iter().rev() {
    steps.push(KeyStep::Up(*vk));
  }
  Ok(steps)
}

/// 滚动页数 → wheel delta（**clamp** 0..=100 页，brief 裁定 2；不报错）。
pub fn wheel_delta_for_pages(pages: i32) -> AxResult<i32> {
  let clamped = pages.clamp(0, 100);
  Ok(clamped * WHEEL_DELTA as i32)
}

/// event 路径前台校验：`GetForegroundWindow() != hwnd` → `foreground_required`。
/// 必须在任何输入构造/投递之前调用（controller：前台违规零下发）。
pub fn ensure_foreground(window_id: u32) -> AxResult<()> {
  let want = HWND(window_id as usize as *mut c_void);
  // unsafe 依据（块级）：GetForegroundWindow 为 FFI 裸调用。
  let got = unsafe { GetForegroundWindow() };
  if got != want {
    return Err(AxError::new(
      "foreground_required",
      format!("目标窗口 {window_id} 不在前台，event 注入未下发"),
    ));
  }
  Ok(())
}

/// modifiers 字段解析（`+`/`,`/空白分隔，每段须为可按住的 keysym；空串 = 无修饰）。
pub(crate) fn parse_modifiers(spec: &str) -> AxResult<Vec<u16>> {
  let mut out: Vec<u16> = Vec::new();
  for token in spec.split(|c: char| c == '+' || c == ',' || c.is_whitespace()) {
    if token.is_empty() {
      continue;
    }
    match resolve_token(token)? {
      TokenKey::Vk { vk, .. } => {
        if !out.contains(&vk) {
          out.push(vk);
        }
      }
      TokenKey::Unicode(_) => {
        return Err(AxError::new(
          "invalid_request",
          format!("修饰键不能是无 VK 字符: {token}"),
        ));
      }
    }
  }
  Ok(out)
}

/// 扩展键（需置 `KEYEVENTF_EXTENDEDKEY`，否则控制台等目标会误判）。
fn is_extended(vk: u16) -> bool {
  matches!(
    vk,
    0xA3 // VK_RCONTROL
      | 0xA5 // VK_RMENU
      | 0x5B // VK_LWIN
      | 0x5C // VK_RWIN
      | 0x2D // VK_INSERT
      | 0x2E // VK_DELETE
      | 0x24 // VK_HOME
      | 0x23 // VK_END
      | 0x21 // VK_PRIOR
      | 0x22 // VK_NEXT
      | 0x25 | 0x26 | 0x27 | 0x28 // 方向键
      | 0x90 // VK_NUMLOCK
      | 0x2C // VK_SNAPSHOT
      | 0x6F // VK_DIVIDE
  )
}

/// KEYDOWN / KEYUP 事件。
pub(crate) fn key_input(vk: u16, up: bool) -> INPUT {
  let mut flags = KEYBD_EVENT_FLAGS(0);
  if up {
    flags |= KEYEVENTF_KEYUP;
  }
  if is_extended(vk) {
    flags |= KEYEVENTF_EXTENDEDKEY;
  }
  INPUT {
    r#type: INPUT_KEYBOARD,
    Anonymous: INPUT_0 {
      ki: KEYBDINPUT {
        wVk: VIRTUAL_KEY(vk),
        wScan: 0,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
      },
    },
  }
}

/// `KEYEVENTF_UNICODE` 事件（wVk=0、wScan=UTF-16 码元）。
fn unicode_input(unit: u16, up: bool) -> INPUT {
  let mut flags = KEYEVENTF_UNICODE;
  if up {
    flags |= KEYEVENTF_KEYUP;
  }
  INPUT {
    r#type: INPUT_KEYBOARD,
    Anonymous: INPUT_0 {
      ki: KEYBDINPUT {
        wVk: VIRTUAL_KEY(0),
        wScan: unit,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
      },
    },
  }
}

/// char → UTF-16 码元（BMP 直出；非 BMP 拆代理对）。
/// WHY 手写：rustc 1.99 的 `char::encode_utf16` 签名要求 `&mut [u16]` 缓冲（编译期实测），
/// 迭代器语义在此不存在——手动编码两行且无歧义。
fn utf16_units(c: char) -> Vec<u16> {
  let cp = c as u32;
  if cp <= 0xFFFF {
    vec![cp as u16]
  } else {
    let v = cp - 0x1_0000;
    vec![0xD800 + (v >> 10) as u16, 0xDC00 + (v & 0x3FF) as u16]
  }
}

/// 一个 `KeyStep` 的**按下侧**事件（hold 分压用：Tap/Unicode 只出 down 半边）。
pub(crate) fn step_press(step: &KeyStep) -> Vec<INPUT> {
  match step {
    KeyStep::Down(vk) | KeyStep::Tap(vk) => vec![key_input(*vk, false)],
    KeyStep::Up(_) => vec![],
    // Unicode 按 UTF-16 码元逐个 down（代理对两个码元都按下，与 release 对齐）。
    KeyStep::Unicode(c) => utf16_units(*c)
      .into_iter()
      .map(|u| unicode_input(u, false))
      .collect(),
  }
}

/// 一个 `KeyStep` 的**抬起侧**事件（hold 分压用）。
pub(crate) fn step_release(step: &KeyStep) -> Vec<INPUT> {
  match step {
    KeyStep::Up(vk) | KeyStep::Tap(vk) => vec![key_input(*vk, true)],
    KeyStep::Down(_) => vec![],
    KeyStep::Unicode(c) => utf16_units(*c)
      .into_iter()
      .map(|u| unicode_input(u, true))
      .collect(),
  }
}

/// 一个 `KeyStep` 的原子事件（无 hold：整段序列进同一 SendInput 批次）。
pub(crate) fn step_atomic(step: &KeyStep) -> Vec<INPUT> {
  match step {
    KeyStep::Down(vk) => vec![key_input(*vk, false)],
    KeyStep::Up(vk) => vec![key_input(*vk, true)],
    KeyStep::Tap(vk) => vec![key_input(*vk, false), key_input(*vk, true)],
    KeyStep::Unicode(c) => utf16_units(*c)
      .into_iter()
      .flat_map(|u| [unicode_input(u, false), unicode_input(u, true)])
      .collect(),
  }
}

/// `type_text` 的整串注入事件：逐 UTF-16 码元 down+up（`KEYEVENTF_UNICODE`；
/// IME 组合按 spec 为一期范围外）。
pub(crate) fn type_text_inputs(text: &str) -> Vec<INPUT> {
  text
    .chars()
    .flat_map(utf16_units)
    .flat_map(|u| [unicode_input(u, false), unicode_input(u, true)])
    .collect()
}

/// NUL 结尾 UTF-16 序列（UIA `SetValue(PCWSTR)` 等宽字符入参用）。
pub(crate) fn wide_z(text: &str) -> Vec<u16> {
  text
    .chars()
    .flat_map(utf16_units)
    .chain(std::iter::once(0))
    .collect()
}

/// 虚拟屏原点与尺寸（多显示器；原点可为负）。
fn virtual_screen() -> (i32, i32, i32, i32) {
  // unsafe 依据（块级）：GetSystemMetrics FFI。
  unsafe {
    (
      GetSystemMetrics(SM_XVIRTUALSCREEN),
      GetSystemMetrics(SM_YVIRTUALSCREEN),
      GetSystemMetrics(SM_CXVIRTUALSCREEN),
      GetSystemMetrics(SM_CYVIRTUALSCREEN),
    )
  }
}

/// 屏幕绝对坐标 → `MOUSEEVENTF_ABSOLUTE|VIRTUALDESK` 归一化 0..=65535
/// （`(x-vx)*65535/(vw-1)`，减虚拟屏原点；brief 公式按原点 0 简写，多屏必须平移）。
fn normalize(x: i32, y: i32) -> (i32, i32) {
  let (vx, vy, vw, vh) = virtual_screen();
  let norm = |v: i32, origin: i32, span: i32| -> i32 {
    let span = span.max(2) - 1; // w-1；退化尺寸防 0 除
    let rel = i64::from(v - origin).clamp(0, i64::from(span));
    i32::try_from(rel * 65535 / i64::from(span)).unwrap_or(0)
  };
  (norm(x, vx, vw), norm(y, vy, vh))
}

/// 移动到 (x,y) 的绝对坐标鼠标事件。
pub(crate) fn mouse_move_to(x: i32, y: i32) -> INPUT {
  let (dx, dy) = normalize(x, y);
  INPUT {
    r#type: INPUT_MOUSE,
    Anonymous: INPUT_0 {
      mi: MOUSEINPUT {
        dx,
        dy,
        mouseData: 0,
        dwFlags: MOUSE_EVENT_FLAGS(
          (MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK).0,
        ),
        time: 0,
        dwExtraInfo: 0,
      },
    },
  }
}

/// 按键名 → (按下, 抬起) 鼠标事件标志；未知按键 → invalid_request。
pub(crate) fn mouse_button_flags(button: &str) -> AxResult<(MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS)> {
  match button {
    "left" => Ok((MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)),
    "right" => Ok((MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)),
    "middle" => Ok((MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP)),
    other => Err(AxError::new(
      "invalid_request",
      format!("未知鼠标按键: {other}（期望 left|right|middle）"),
    )),
  }
}

/// 按鼠标事件标志构造按下/抬起事件；坐标由单独的 move 事件携带。
pub(crate) fn mouse_button_input(flags: MOUSE_EVENT_FLAGS) -> INPUT {
  INPUT {
    r#type: INPUT_MOUSE,
    Anonymous: INPUT_0 {
      mi: MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: 0,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
      },
    },
  }
}

/// 滚轮事件（delta 带符号；下/左为负）。`vertical=false` 走 `MOUSEEVENTF_HWHEEL`（右向）。
pub(crate) fn mouse_wheel_input(delta: i32, vertical: bool) -> INPUT {
  let flags = if vertical {
    MOUSEEVENTF_WHEEL
  } else {
    MOUSEEVENTF_HWHEEL
  };
  INPUT {
    r#type: INPUT_MOUSE,
    Anonymous: INPUT_0 {
      mi: MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: delta as u32,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
      },
    },
  }
}

/// dispatched 三态的跨批次累计器。
///
/// 每次 `SendInput` 调用记一对 (期望, 实发)：合计相等 → `dispatched`；合计 0 →
/// `not_dispatched`；介于其间（部分批次成功/部分条数）→ `unknown`。
pub(crate) struct InputTally {
  expected: u64,
  sent: u64,
}

impl InputTally {
  pub(crate) fn new() -> Self {
    Self {
      expected: 0,
      sent: 0,
    }
  }

  /// 投递一批 INPUT 并累计；空批次不调 SendInput（不产生 0 条误判）。
  pub(crate) fn push(&mut self, inputs: &[INPUT]) {
    if inputs.is_empty() {
      return;
    }
    self.expected += inputs.len() as u64;
    // unsafe 依据（块级）：SendInput FFI；inputs 生命周期覆盖调用。
    let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) } as u64;
    self.sent += n;
  }

  /// 汇总裁定（无任何期望 = 无事可做 → not_dispatched）。
  pub(crate) fn finish(&self) -> &'static str {
    if self.expected == 0 {
      "not_dispatched"
    } else if self.sent == self.expected {
      "dispatched"
    } else if self.sent == 0 {
      "not_dispatched"
    } else {
      "unknown"
    }
  }
}
