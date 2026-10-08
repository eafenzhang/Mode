//! `perform` 动作分发：9 种 kind → event 注入（SendInput / 剪贴板）或 UIA pattern（STA 队列）。
//!
//! 分派纪律（controller 指引 + brief Produces）：
//! - **event 路径**（click/click_drag/scroll/key/type_text/paste + set_value 的事件兜底）：
//!   `ensure_foreground` 必须先于一切输入构造/投递——前台违规 → `foreground_required`
//!   且零下发（参数校验更早：非法参数不查前台）。
//! - **UIA 路径**（set_value 的 ValuePattern、select_text 的 TextPattern、action 的
//!   Invoke/Toggle/…）：跨进程 pattern 调用与 observe/capture 共用同一条 STA 队列——
//!   调用前 `uia_preflight` 复用 `IsHungAppWindow` 预检（参照 `capture::should_blit_first`
//!   的理由：挂起窗口会占死共享队列），目标挂起 → 不发起调用、错误为 `timeout`。
//!   SendInput/剪贴板不触目标消息泵，在调用方线程同步执行（不进 STA 队列）。
//! - dispatched 三态见 `input::InputTally`；UIA 成功 → `dispatched`、元素消亡 →
//!   `element_unavailable`、无 pattern → `not_settable`/`not_selectable`/`action_unavailable`。
//!
//! 坐标/元素解析分工（brief Step 4 决策）：helper JS 负责索引→坐标的会话级解析，
//! addon 侧 `elementIndex` 由本文件在 STA 线程内按 observe 同序重枚举解析（UIA 路径），
//! 坐标点（`set_value` 的「x,y 焦点」变体）直接走事件聚焦。
use crate::error::{AxError, AxResult};
use crate::{clipboard, input, uia_thread};
use serde_json::Value;
use std::ffi::c_void;
use std::time::Duration;
use windows::core::BSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
  IUIAutomation, IUIAutomationElement, IUIAutomationExpandCollapsePattern,
  IUIAutomationInvokePattern, IUIAutomationLegacyIAccessiblePattern, IUIAutomationScrollItemPattern,
  IUIAutomationSelectionItemPattern, IUIAutomationTextPattern, IUIAutomationTogglePattern,
  IUIAutomationValuePattern, TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start,
  TextUnit_Character, TreeScope_Subtree, UIA_E_ELEMENTNOTAVAILABLE, UIA_ExpandCollapsePatternId,
  UIA_InvokePatternId, UIA_LegacyIAccessiblePatternId, UIA_ScrollItemPatternId,
  UIA_SelectionItemPatternId, UIA_TextPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
};

// 双击/拖拽中缝与粘贴 settle、聚焦生效窗（brief Step 3 指定 50/150，聚焦窗为实现决策）。
const CLICK_GAP_MS: u64 = 50;
const PASTE_SETTLE_MS: u64 = 150;
const FOCUS_SETTLE_MS: u64 = 50;

/// UIA 路径的元素定位方式（payload `set_value {elementIndex|"x,y"焦点, value}` 二选一）。
#[derive(Debug)]
pub enum ElementTarget {
  /// observe 索引：STA 线程内按与 observe 相同的 FindAll 序重枚举取元素。
  Index(u32),
  /// 绝对屏幕坐标：helper 已解析好的聚焦点（事件点击聚焦，无 UIA 调用）。
  Point { x: i32, y: i32 },
}

/// 九种动作的内部请求（napi payload 经 `parse_req` 落到这里；字段语义见 brief 形状表）。
#[derive(Debug)]
pub enum Req<'a> {
  Click {
    x: i32,
    y: i32,
    button: &'a str,
    click_count: u32,
    modifiers: &'a str,
  },
  ClickDrag {
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
    modifiers: &'a str,
  },
  Scroll {
    x: i32,
    y: i32,
    direction: &'a str,
    amount: f64,
  },
  Key {
    chord: &'a str,
    repeat: u32,
    hold_ms: u32,
  },
  TypeText {
    text: &'a str,
  },
  SetValue {
    target: ElementTarget,
    value: &'a str,
  },
  SelectText {
    element_index: u32,
    start: i32,
    length: i32,
  },
  Action {
    element_index: u32,
    action: &'a str,
  },
  Paste {
    text: &'a str,
  },
}

/// 对外入口（测试与 napi 共用）。参数校验 → 前台校验（event）/ 挂起预检（UIA）→ 注入。
pub fn perform(req: &Req<'_>, window_id: u32) -> AxResult<String> {
  match req {
    Req::Click {
      x,
      y,
      button,
      click_count,
      modifiers,
    } => click(window_id, *x, *y, button, *click_count, modifiers),
    Req::ClickDrag {
      from_x,
      from_y,
      to_x,
      to_y,
      modifiers,
    } => click_drag(window_id, *from_x, *from_y, *to_x, *to_y, modifiers),
    Req::Scroll {
      x,
      y,
      direction,
      amount,
    } => scroll(window_id, *x, *y, direction, *amount),
    Req::Key {
      chord,
      repeat,
      hold_ms,
    } => key(window_id, chord, *repeat, *hold_ms),
    Req::TypeText { text } => type_text(window_id, text),
    Req::SetValue { target, value } => set_value(window_id, target, value),
    Req::SelectText {
      element_index,
      start,
      length,
    } => select_text(window_id, *element_index, *start, *length),
    Req::Action {
      element_index,
      action: name,
    } => action(window_id, *element_index, name),
    Req::Paste { text } => paste(window_id, text),
  }
}

/// 短 settle（指定位数）。
fn settle(ms: u64) {
  std::thread::sleep(Duration::from_millis(ms));
}

// ————————————————————————— event 路径 —————————————————————————

/// 点击：mods↓ → 移动 → 按下 → 抬起 → mods↑；click_count>1 时次起每击间隔 50ms。
fn click(
  window_id: u32,
  x: i32,
  y: i32,
  button: &str,
  click_count: u32,
  modifiers: &str,
) -> AxResult<String> {
  // —— 校验先行（先于前台校验与任何 INPUT 构造）。
  let (down, up) = input::mouse_button_flags(button)?;
  let mods = input::parse_modifiers(modifiers)?;
  if click_count == 0 {
    return Err(AxError::new("invalid_request", "clickCount 至少为 1"));
  }
  // —— 前台闸门：违规即零下发。
  input::ensure_foreground(window_id)?;
  let mut tally = input::InputTally::new();
  for i in 0..click_count {
    if i > 0 {
      settle(CLICK_GAP_MS); // 双击中缝（brief 指定 50ms）
    }
    let mut batch = Vec::with_capacity(mods.len() * 2 + 3);
    if i == 0 {
      for m in &mods {
        batch.push(input::key_input(*m, false));
      }
    }
    batch.push(input::mouse_move_to(x, y));
    batch.push(input::mouse_button_input(down));
    batch.push(input::mouse_button_input(up));
    if i + 1 == click_count {
      for m in mods.iter().rev() {
        batch.push(input::key_input(*m, true));
      }
    }
    tally.push(&batch);
  }
  Ok(tally.finish().to_string())
}

/// 拖拽（left 按钮）：mods↓ → 移动起点 → 按下 →（50ms 中缝）→ 移动终点 → 抬起 → mods↑。
fn click_drag(
  window_id: u32,
  from_x: i32,
  from_y: i32,
  to_x: i32,
  to_y: i32,
  modifiers: &str,
) -> AxResult<String> {
  let mods = input::parse_modifiers(modifiers)?;
  input::ensure_foreground(window_id)?;
  let (down, up) = input::mouse_button_flags("left")?;
  let mut tally = input::InputTally::new();

  let mut press = Vec::with_capacity(mods.len() + 2);
  for m in &mods {
    press.push(input::key_input(*m, false));
  }
  press.push(input::mouse_move_to(from_x, from_y));
  press.push(input::mouse_button_input(down));
  tally.push(&press);

  settle(CLICK_GAP_MS); // 拖拽按下与移动的中缝（brief：拖拽中间 50ms）

  let mut release = Vec::with_capacity(mods.len() + 2);
  release.push(input::mouse_move_to(to_x, to_y));
  release.push(input::mouse_button_input(up));
  for m in mods.iter().rev() {
    release.push(input::key_input(*m, true));
  }
  tally.push(&release);
  Ok(tally.finish().to_string())
}

/// 滚动：先定位光标，再滚轮（垂直 WHEEL / 水平 HWHEEL），delta=120×页数（clamp 后带符号）。
fn scroll(window_id: u32, x: i32, y: i32, direction: &str, amount: f64) -> AxResult<String> {
  // —— 校验先行。
  let (vertical, sign) = match direction {
    "up" => (true, 1),
    "down" => (true, -1),
    "right" => (false, 1),
    "left" => (false, -1),
    other => {
      return Err(AxError::new(
        "invalid_request",
        format!("未知滚动方向: {other}（期望 up|down|left|right）"),
      ))
    }
  };
  if !amount.is_finite() {
    return Err(AxError::new("invalid_request", "scroll amount 非有限数"));
  }
  let pages = amount.round() as i32; // f64→i32 饱和转换，越界由 clamp 收口
  let delta = input::wheel_delta_for_pages(pages)? * sign;
  if delta == 0 {
    // 0 页（含负数 clamp 到 0）：无轮事件可发——零下发如实上报。
    return Ok("not_dispatched".to_string());
  }
  input::ensure_foreground(window_id)?;
  let mut tally = input::InputTally::new();
  tally.push(&[input::mouse_move_to(x, y)]);
  tally.push(&[input::mouse_wheel_input(delta, vertical)]);
  Ok(tally.finish().to_string())
}

/// 按键：chord 展开 → repeat 次；hold_ms>0 时按下/抬起分批、中间保持。
fn key(window_id: u32, chord: &str, repeat: u32, hold_ms: u32) -> AxResult<String> {
  let steps = input::resolve_chord(chord)?; // 校验先行
  if repeat == 0 {
    return Ok("not_dispatched".to_string());
  }
  input::ensure_foreground(window_id)?;
  let mut tally = input::InputTally::new();
  for _ in 0..repeat {
    if hold_ms > 0 {
      let press: Vec<_> = steps.iter().flat_map(input::step_press).collect();
      let release: Vec<_> = steps.iter().flat_map(input::step_release).collect();
      tally.push(&press);
      settle(u64::from(hold_ms));
      tally.push(&release);
    } else {
      let batch: Vec<_> = steps.iter().flat_map(input::step_atomic).collect();
      tally.push(&batch);
    }
  }
  Ok(tally.finish().to_string())
}

/// 键入：逐 UTF-16 码元 `KEYEVENTF_UNICODE`（IME 组合一期范围外）。
fn type_text(window_id: u32, text: &str) -> AxResult<String> {
  if text.is_empty() {
    return Ok("not_dispatched".to_string());
  }
  input::ensure_foreground(window_id)?;
  let mut tally = input::InputTally::new();
  tally.push(&input::type_text_inputs(text));
  Ok(tally.finish().to_string())
}

/// 粘贴：前台闸门 → 保存 CF_UNICODETEXT → 写入 → Ctrl+V → settle 150ms → 还原。
/// `ClipboardBackup` RAII：写入/发送中途失败或 panic 展开，Drop 同样还原剪贴板。
fn paste(window_id: u32, text: &str) -> AxResult<String> {
  if text.is_empty() {
    return Ok("not_dispatched".to_string());
  }
  let steps = input::resolve_chord("ctrl+v")?; // 校验先行（常量 chord，防御性保留）
  // 前台闸门必须先于剪贴板写入——剪贴板也是注入面，违规时零改动。
  input::ensure_foreground(window_id)?;
  let backup = clipboard::ClipboardBackup::capture()?;
  clipboard::write_text(text)?;
  let mut tally = input::InputTally::new();
  tally.push(&steps.iter().flat_map(input::step_atomic).collect::<Vec<_>>());
  settle(PASTE_SETTLE_MS); // 等目标消费剪贴板后再还原
  drop(backup); // 显式还原（正常路径；panic 路径由 Drop 兜底）
  Ok(tally.finish().to_string())
}

/// 焦点元素「全选 + 键入」（brief 指定辅助；set_value 事件兜底调用）。
/// 空串 = 全选后 Delete 清空（只选不键入不会改变值）。
///
/// 全选 = `ctrl+Home`（归位文首）+ `ctrl+shift+End`（选到文末）。
/// WHY 不用 `ctrl+a`：经典 Win32 EDIT 实测**忽略** Ctrl+A（EM_GETSEL 探针：修饰/按键
/// 投递本身正常——shift+Left、ctrl+Home、ctrl+shift+End 都生效——唯独 Ctrl+A 无选区）；
/// Home/End 导航键是 EDIT 与 RichEdit/Chromium 等现代控件的共有键，两条 chord 通吃。
pub fn select_all_and_type(window_id: u32, text: &str) -> AxResult<String> {
  input::ensure_foreground(window_id)?; // 自身也是注入入口 → 自检
  let mut tally = input::InputTally::new();
  for chord in ["ctrl+Home", "ctrl+shift+End"] {
    let steps = input::resolve_chord(chord)?;
    tally.push(&steps.iter().flat_map(input::step_atomic).collect::<Vec<_>>());
  }
  if text.is_empty() {
    let steps = input::resolve_chord("Delete")?;
    tally.push(&steps.iter().flat_map(input::step_atomic).collect::<Vec<_>>());
    return Ok(tally.finish().to_string());
  }
  settle(FOCUS_SETTLE_MS); // 全选 → 键入的小窗
  tally.push(&input::type_text_inputs(text));
  Ok(tally.finish().to_string())
}

// ————————————————————————— UIA 路径（STA 队列） —————————————————————————

/// UIA 路径前置：目标挂起 → 不发起跨进程 pattern 调用，错误 `timeout`（可重试语义）。
/// 复用 `capture::should_blit_first` 的 `IsHungAppWindow` 判定（同一风险：挂起窗口的
/// 消息泵不应答会把共享 STA 队列占死，连坐 observe/capture）。
fn uia_preflight(window_id: u32) -> AxResult<()> {
  let hwnd = HWND(window_id as usize as *mut c_void);
  if crate::capture::should_blit_first(hwnd) {
    return Err(AxError::new(
      "timeout",
      format!("目标窗口 {window_id} 挂起，UIA 调用未发起"),
    ));
  }
  Ok(())
}

/// UIA 调用错误映射：`UIA_E_ELEMENTNOTAVAILABLE` → `element_unavailable`，其余 → internal。
fn map_uia_err(e: windows::core::Error) -> AxError {
  if (e.code().0 as u32) == UIA_E_ELEMENTNOTAVAILABLE {
    AxError::new("element_unavailable", format!("元素已不可用: {e}"))
  } else {
    AxError::internal(format!("UIA pattern 调用失败: {e}"))
  }
}

/// 按 observe 同序解析元素：`ElementFromHandle` → `FindAll(subtree, true)` → `GetElement(index)`
/// （index 即 observe 结果里的 `index`；越界/消亡 → `element_unavailable`）。
/// 仅在 STA 线程执行（由 `submit_perform` 保证）。
fn resolve_element(
  auto: &IUIAutomation,
  window_id: u32,
  index: u32,
) -> AxResult<IUIAutomationElement> {
  let hwnd = HWND(window_id as usize as *mut c_void);
  let idx = i32::try_from(index)
    .map_err(|_| AxError::new("element_unavailable", format!("元素索引越界: {index}")))?;
  // unsafe 依据（块级）：UIA 接口方法为 COM vtable 裸调用；调用方 uia_thread 保证
  // 本函数只在进程单例 STA 线程执行，元素只在本线程存活使用。
  unsafe {
    let root = auto.ElementFromHandle(hwnd).map_err(|e| {
      AxError::new(
        "element_unavailable",
        format!("ElementFromHandle({window_id}) failed: {e}"),
      )
    })?;
    let cond = auto
      .CreateTrueCondition()
      .map_err(|e| AxError::internal(format!("CreateTrueCondition failed: {e}")))?;
    let arr = root
      .FindAll(TreeScope_Subtree, &cond)
      .map_err(|e| AxError::new("element_unavailable", format!("FindAll failed: {e}")))?;
    arr.GetElement(idx).map_err(|e| {
      AxError::new(
        "element_unavailable",
        format!("元素 {index} 不存在: {e}"),
      )
    })
  }
}

/// set_value 的 STA 阶段结果：ValuePattern 直接设值成功，或无 pattern（带回包围矩走事件兜底）。
enum SetValueOutcome {
  Set,
  NoPattern { bounds: [i32; 4] },
}

/// `set_value`：优先 UIA ValuePattern（后台可用）；无 pattern → 聚焦点点击 + 全选 + 键入
/// 的事件兜底（spec：两者均不可 → `not_settable`；兜底需要前台，违规 → `foreground_required`）。
fn set_value(window_id: u32, target: &ElementTarget, value: &str) -> AxResult<String> {
  match target {
    ElementTarget::Point { x, y } => event_set_value(window_id, *x, *y, value),
    ElementTarget::Index(index) => {
      uia_preflight(window_id)?;
      let index = *index;
      // 闭包进 STA 线程 → 'static：value 拷贝为 owned。
      let owned = value.to_string();
      let outcome = uia_thread::submit_perform(move |auto: AxResult<&IUIAutomation>| {
        let auto = auto?;
        let el = resolve_element(auto, window_id, index)?;
        let bounds = unsafe { el.CurrentBoundingRectangle() }
          .map(|r| [r.left, r.top, r.right, r.bottom])
          .unwrap_or([0, 0, 0, 0]);
        let pattern = match unsafe {
          el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
        } {
          Ok(p) => p,
          Err(_) => return Ok(SetValueOutcome::NoPattern { bounds }),
        };
        let readonly = unsafe { pattern.CurrentIsReadOnly() }.map_err(map_uia_err)?;
        if readonly.as_bool() {
          return Err(AxError::new("not_settable", "元素值只读"));
        }
        // ValuePattern::SetValue 收 BSTR（Param<BSTR> 需 &BSTR：CloneType blanket impl）。
        unsafe { pattern.SetValue(&BSTR::from(owned.as_str())) }.map_err(map_uia_err)?;
        Ok(SetValueOutcome::Set)
      })?;
      match outcome {
        SetValueOutcome::Set => Ok("dispatched".to_string()),
        SetValueOutcome::NoPattern { bounds } => {
          if bounds[2] <= bounds[0] || bounds[3] <= bounds[1] {
            // 无 pattern 且无有效包围矩：事件兜底也没有聚焦点。
            return Err(AxError::new(
              "not_settable",
              "元素无 ValuePattern 且包围矩为空，无法聚焦键入",
            ));
          }
          event_set_value(
            window_id,
            (bounds[0] + bounds[2]) / 2,
            (bounds[1] + bounds[3]) / 2,
            value,
          )
        }
      }
    }
  }
}

/// set_value 的事件兜底：点击聚焦（left, 1 击）→ 全选 → 键入。
fn event_set_value(window_id: u32, x: i32, y: i32, value: &str) -> AxResult<String> {
  // click 内含参数校验与前台闸门；未真正发出点击（not_dispatched/unknown）时不叠加键入，
  // 避免把输入打到未确认的焦点上。
  let clicked = click(window_id, x, y, "left", 1, "")?;
  if clicked != "dispatched" {
    return Ok(clicked);
  }
  settle(FOCUS_SETTLE_MS);
  select_all_and_type(window_id, value)
}

/// `select_text`：UIA TextPattern 的 range Select（无该 pattern → `not_selectable`；
/// start/length 越界 → `invalid_request`）。
fn select_text(window_id: u32, element_index: u32, start: i32, length: i32) -> AxResult<String> {
  // 校验先行（先于挂起预检与 STA 投递）。
  if start < 0 || length < 0 {
    return Err(AxError::new(
      "invalid_request",
      "select_text 的 start/length 不能为负",
    ));
  }
  uia_preflight(window_id)?;
  uia_thread::submit_perform(move |auto: AxResult<&IUIAutomation>| {
    let auto = auto?;
    let el = resolve_element(auto, window_id, element_index)?;
    let pattern = unsafe { el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
      .map_err(|_| {
        AxError::new("not_selectable", "元素不支持 TextPattern（无法选中文本）")
      })?;
    // unsafe 依据（块级）：TextRange COM 方法；range 立即用于 Select，不跨线程存活。
    unsafe {
      let doc = pattern.DocumentRange().map_err(map_uia_err)?;
      let range = doc.Clone().map_err(map_uia_err)?;
      let moved = range.Move(TextUnit_Character, start).map_err(map_uia_err)?;
      if moved != start {
        return Err(AxError::new(
          "invalid_request",
          format!("select_text start 越界（文本仅移动了 {moved} 字符）"),
        ));
      }
      // 目标 [start, start+length)。Move 之后 range 的形状因提供者而异（RichEdit50W 实测
      // 折叠成 [start, start+1)，其他实现可能保留到文末）——用 CompareEndpoints 取 End 端
      // 相对文档起点的**位置**，按差分移动 End：两种形态都收敛到同一目标。差分被夹断
      // （ext != delta）= 越过文末 → invalid_request（保守失败，不静默截断）。
      let end_pos = range
        .CompareEndpoints(TextPatternRangeEndpoint_End, &doc, TextPatternRangeEndpoint_Start)
        .map_err(map_uia_err)?;
      let target_end = start.saturating_add(length);
      let delta = target_end.saturating_sub(end_pos);
      if delta != 0 {
        let ext = range
          .MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, delta)
          .map_err(map_uia_err)?;
        if ext != delta {
          return Err(AxError::new(
            "invalid_request",
            format!("select_text length 越界（仅移动了 {ext} 字符）"),
          ));
        }
      }
      range.Select().map_err(map_uia_err)?;
    }
    Ok("dispatched".to_string())
  })
}

/// `perform_action`：只接受可映射的具名动作或元素 Legacy 默认动作原文（与 observe 的
/// actions 同源），映射不上/无 pattern → `action_unavailable`。
fn action(window_id: u32, element_index: u32, name: &str) -> AxResult<String> {
  if name.is_empty() {
    return Err(AxError::new("invalid_request", "action 不能为空"));
  }
  uia_preflight(window_id)?;
  let name = name.to_string();
  uia_thread::submit_perform(move |auto: AxResult<&IUIAutomation>| {
    let auto = auto?;
    let el = resolve_element(auto, window_id, element_index)?;
    let unavailable = |what: &str| AxError::new("action_unavailable", what.to_string());
    // unsafe 依据（块级）：pattern 取用与调用均为 COM vtable 裸调用，仅本线程内存活。
    unsafe {
      match name.as_str() {
        "press" => {
          let p = el
            .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            .map_err(|_| unavailable("元素无 Invoke pattern（press 不可用）"))?;
          p.Invoke().map_err(map_uia_err)?;
        }
        "toggle" => {
          let p = el
            .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
            .map_err(|_| unavailable("元素无 Toggle pattern（toggle 不可用）"))?;
          p.Toggle().map_err(map_uia_err)?;
        }
        "expand" | "collapse" => {
          let p = el
            .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
              UIA_ExpandCollapsePatternId,
            )
            .map_err(|_| unavailable("元素无 ExpandCollapse pattern"))?;
          if name == "expand" {
            p.Expand().map_err(map_uia_err)?;
          } else {
            p.Collapse().map_err(map_uia_err)?;
          }
        }
        "select" => {
          let p = el
            .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
              UIA_SelectionItemPatternId,
            )
            .map_err(|_| unavailable("元素无 SelectionItem pattern（select 不可用）"))?;
          p.Select().map_err(map_uia_err)?;
        }
        "scroll_into_view" => {
          let p = el
            .GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(UIA_ScrollItemPatternId)
            .map_err(|_| unavailable("元素无 ScrollItem pattern（scroll_into_view 不可用）"))?;
          p.ScrollIntoView().map_err(map_uia_err)?;
        }
        other => {
          // 契约「只接受该元素 actions 列出的动作」：Legacy 默认动作按观察时同源的
          // CurrentDefaultAction 原文精确匹配，匹配不上 → action_unavailable（不乱猜）。
          let legacy = el
            .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
              UIA_LegacyIAccessiblePatternId,
            )
            .map_err(|_| unavailable(format!("元素未列出动作: {other}").as_str()))?;
          let default_action = legacy
            .CurrentDefaultAction()
            .map(|t| t.to_string())
            .unwrap_or_default();
          if default_action != other {
            return Err(unavailable(format!("元素未列出动作: {other}").as_str()));
          }
          legacy.DoDefaultAction().map_err(map_uia_err)?;
        }
      }
    }
    Ok("dispatched".to_string())
  })
}

// ————————————————————————— payload 解析（napi 面） —————————————————————————

fn missing(key: &str) -> AxError {
  AxError::new("invalid_request", format!("payload 缺少字段 {key}"))
}

fn p_str<'a>(payload: &'a Value, key: &str) -> AxResult<&'a str> {
  payload
    .get(key)
    .and_then(Value::as_str)
    .ok_or_else(|| missing(key))
}

/// 可选字符串：缺省 → default；类型不对 → invalid_request。
fn p_str_or<'a>(payload: &'a Value, key: &str, default: &'a str) -> AxResult<&'a str> {
  match payload.get(key) {
    None => Ok(default),
    Some(v) => v.as_str().ok_or_else(|| missing(key)),
  }
}

/// JSON 数字（整数值）→ i64；缺省/非整数 → invalid_request。
fn p_int(payload: &Value, key: &str) -> AxResult<i64> {
  payload
    .get(key)
    .and_then(Value::as_f64)
    .filter(|f| f.fract() == 0.0)
    .map(|f| f as i64)
    .ok_or_else(|| missing(key))
}

fn p_i32(payload: &Value, key: &str) -> AxResult<i32> {
  i32::try_from(p_int(payload, key)?)
    .map_err(|_| AxError::new("invalid_request", format!("字段 {key} 超出 i32 范围")))
}

fn p_u32(payload: &Value, key: &str) -> AxResult<u32> {
  u32::try_from(p_int(payload, key)?)
    .map_err(|_| AxError::new("invalid_request", format!("字段 {key} 超出 u32 范围")))
}

fn p_u32_or(payload: &Value, key: &str, default: u32) -> AxResult<u32> {
  match payload.get(key) {
    None => Ok(default),
    Some(_) => p_u32(payload, key),
  }
}

/// napi payload → `Req`。字段形状与缺省策略见各 arm 注释（brief 形状表 + Task 8 消费面）。
/// 借用 `payload` 内的字符串（`Req<'a>` 零拷贝）；数值字段就地转换、越界/缺失 → invalid_request。
pub fn parse_req<'a>(kind: &str, payload: &'a Value) -> AxResult<Req<'a>> {
  if !payload.is_object() {
    return Err(AxError::new(
      "invalid_request",
      "perform payload 必须是对象",
    ));
  }
  match kind {
    "click" => Ok(Req::Click {
      x: p_i32(payload, "x")?,
      y: p_i32(payload, "y")?,
      button: p_str_or(payload, "button", "left")?,
      click_count: p_u32_or(payload, "clickCount", 1)?,
      modifiers: p_str_or(payload, "modifiers", "")?,
    }),
    "click_drag" => Ok(Req::ClickDrag {
      from_x: p_i32(payload, "fromX")?,
      from_y: p_i32(payload, "fromY")?,
      to_x: p_i32(payload, "toX")?,
      to_y: p_i32(payload, "toY")?,
      modifiers: p_str_or(payload, "modifiers", "")?,
    }),
    "scroll" => {
      let amount = match payload.get("amount") {
        None => 1.0,
        Some(v) => v.as_f64().ok_or_else(|| missing("amount"))?,
      };
      Ok(Req::Scroll {
        x: p_i32(payload, "x")?,
        y: p_i32(payload, "y")?,
        direction: p_str(payload, "direction")?,
        amount,
      })
    }
    "key" => Ok(Req::Key {
      chord: p_str(payload, "chord")?,
      repeat: p_u32_or(payload, "repeat", 1)?,
      hold_ms: p_u32_or(payload, "holdMs", 0)?,
    }),
    "type_text" => Ok(Req::TypeText {
      text: p_str(payload, "text")?,
    }),
    "set_value" => {
      let target = if payload.get("elementIndex").is_some() {
        ElementTarget::Index(p_u32(payload, "elementIndex")?)
      } else if payload.get("x").is_some() && payload.get("y").is_some() {
        ElementTarget::Point {
          x: p_i32(payload, "x")?,
          y: p_i32(payload, "y")?,
        }
      } else {
        return Err(AxError::new(
          "invalid_request",
          "set_value 需要 elementIndex 或 x/y 聚焦点之一",
        ));
      };
      Ok(Req::SetValue {
        target,
        value: p_str(payload, "value")?,
      })
    }
    "select_text" => Ok(Req::SelectText {
      element_index: p_u32(payload, "elementIndex")?,
      start: p_i32(payload, "start")?,
      length: p_i32(payload, "length")?,
    }),
    "action" => Ok(Req::Action {
      element_index: p_u32(payload, "elementIndex")?,
      action: p_str(payload, "action")?,
    }),
    "paste" => Ok(Req::Paste {
      text: p_str(payload, "text")?,
    }),
    other => Err(AxError::new(
      "invalid_request",
      format!("未知 perform kind: {other}"),
    )),
  }
}
