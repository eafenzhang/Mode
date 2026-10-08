//! `observe` 原语：枚举指定窗口的 UIA 子树（在 `uia_thread` 的 STA 线程上串行执行）。
//!
//! 语义（spec/plan Task 3 Produces 节）：
//! - `ElementFromHandle` → `FindAll(TreeScope_Subtree, TrueCondition)`，结果为 UIA 树序
//!   （先序深度优先，含根元素本身）；超过上限截断并置 `enumerationComplete=false`。
//! - `focusedIndex` = `GetFocusedElement` 的 RuntimeId 在收集集合中的位置，不匹配为 None。
//! - `actions` = pattern→动作名映射 + LegacyIAccessible DefaultAction 原文（去重、保序）。
//! - 错误映射：窗口/元素消亡 → `element_unavailable`；STA 往返超时由 `uia_thread` 映射
//!   为 `timeout`；其余 → `internal`。
use crate::error::{AxError, AxResult};
use crate::{ElementNapi, ObserveResultNapi};
use std::ffi::c_void;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Ole::{SafeArrayAccessData, SafeArrayDestroy, SafeArrayUnaccessData};
use windows::Win32::UI::Accessibility::*;

/// 元素上限默认值（`ObserveRequestNapi.maxElements` 缺省时使用）。
pub const DEFAULT_MAX_ELEMENTS: u32 = 3000;

/// 对外入口：投递到 STA 命令线程并同步等待（30s 超时在 `uia_thread` 内处理）。
pub fn observe(window_id: u32, max_elements: u32) -> AxResult<ObserveResultNapi> {
  crate::uia_thread::submit_observe(window_id, max_elements)
}

/// STA 线程上执行的实际枚举（由 `uia_thread` 持有的 `IUIAutomation` 调用）。
pub(crate) fn run_observe(
  automation: &IUIAutomation,
  window_id: u32,
  max_elements: u32,
) -> AxResult<ObserveResultNapi> {
  // HWND 在 64 位进程里取低 32 位作窗口 id（与 apps::list_windows 的截断互逆）。
  let hwnd = HWND(window_id as usize as *mut c_void);
  // unsafe 依据（块级）：windows 0.58 把 UIA 接口方法标为 unsafe（COM vtable 裸调用）。
  // 调用方 uia_thread 保证本函数只在进程单例 STA 线程执行（入口已 CoInitializeEx）、
  // automation 与其元素只在该线程使用，且本段只做读取（FindAll/属性/pattern）。
  unsafe {
    let root = automation.ElementFromHandle(hwnd).map_err(|e| {
      AxError::new(
        "element_unavailable",
        format!("ElementFromHandle({window_id}) failed: {e}"),
      )
    })?;
    // 窗口标题取根元素 UIA Name（与窗口标题同源；读不到为 None）。
    let window_title = root.CurrentName().map(|t| t.to_string()).ok();
    let cond = automation.CreateTrueCondition().map_err(|e| {
      AxError::internal(format!("CreateTrueCondition failed: {e}"))
    })?;
    let arr = root
      .FindAll(TreeScope_Subtree, &cond)
      .map_err(|e| AxError::new("element_unavailable", format!("FindAll failed: {e}")))?;
    let total = arr
      .Length()
      .map_err(|e| AxError::new("element_unavailable", format!("element array lost: {e}")))?;
    // 上限先夹到 i32（UIA 计数为 i32），u32→i32 直转会溢出。
    let cap = (max_elements.min(i32::MAX as u32)) as i32;
    let take = total.min(cap);
    let truncating = total > cap;

    let mut elements: Vec<ElementNapi> = Vec::new();
    let mut runtime_ids: Vec<Option<Vec<i32>>> = Vec::new();
    for i in 0..take {
      // 数组中途失效（元素消亡竞态）→ 后续同样取不到，提前结束而非整次失败。
      let el = match arr.GetElement(i) {
        Ok(el) => el,
        Err(_) => break,
      };
      // ControlType 读失败 = 该元素在遍历中途消亡 → 跳过（不产出缺 kind 的残行）。
      let ct = match el.CurrentControlType() {
        Ok(ct) => ct,
        Err(_) => continue,
      };
      runtime_ids.push(runtime_id(&el));
      let index = elements.len() as u32;
      elements.push(ElementNapi {
        index,
        kind: kind_of(ct.0).to_string(),
        // 标题读到文本/读到空串都保留（Some），读失败（竞态）才为 None。
        title: el.CurrentName().map(|t| t.to_string()).ok(),
        value: value_of(&el),
        bounds: match el.CurrentBoundingRectangle() {
          Ok(r) => vec![r.left, r.top, r.right - r.left, r.bottom - r.top],
          Err(_) => vec![0, 0, 0, 0], // 恒 4 个整数的形状约束优先
        },
        actions: actions_of(&el),
        // 读失败按保守值降级：enabled=false（不承诺可操作）、offscreen=true。
        enabled: el.CurrentIsEnabled().map(|b| b.as_bool()).unwrap_or(false),
        offscreen: el
          .CurrentIsOffscreen()
          .map(|b| b.as_bool())
          .unwrap_or(true),
      });
    }

    // focusedIndex = GetFocusedElement 的 RuntimeId 在收集集合中的位置；焦点在别的窗口、
    // RuntimeId 取不到、GetFocusedElement 失败 → None。
    let focused_index = automation
      .GetFocusedElement()
      .ok()
      .and_then(|focused| runtime_id(&focused))
      .and_then(|fid| {
        runtime_ids
          .iter()
          .position(|id| id.as_deref() == Some(fid.as_slice()))
      });

    Ok(ObserveResultNapi {
      window_title,
      focused_index: focused_index.map(|i| i as u32),
      enumeration_complete: !truncating,
      elements,
    })
  }
}

/// ControlType → kind 小写（match 全表映射，未知 → "pane"）。
/// 数值为 UIA_ControlTypeTypeId 常量（UIA_ButtonControlTypeId=50000 … 50040）。
fn kind_of(control_type: i32) -> &'static str {
  match control_type {
    50000 => "button",
    50001 => "calendar",
    50002 => "checkbox",
    50003 => "combo",
    50004 => "edit",
    50005 => "hyperlink",
    50006 => "image",
    50007 => "listitem",
    50008 => "list",
    50009 => "menu",
    50010 => "menubar",
    50011 => "menuitem",
    50012 => "progressbar",
    50013 => "radiobutton",
    50014 => "scrollbar",
    50015 => "slider",
    50016 => "spinner",
    50017 => "statusbar",
    50018 => "tab",
    50019 => "tabitem",
    50020 => "text",
    50021 => "toolbar",
    50022 => "tooltip",
    50023 => "tree",
    50024 => "treeitem",
    50025 => "custom",
    50026 => "group",
    50027 => "thumb",
    50028 => "datagrid",
    50029 => "dataitem",
    50030 => "document",
    50031 => "splitbutton",
    50032 => "window",
    50033 => "pane",
    50034 => "header",
    50035 => "headeritem",
    50036 => "table",
    50037 => "titlebar",
    50038 => "separator",
    50039 => "semantizoom",
    50040 => "appbar",
    _ => "pane",
  }
}

/// 按 Produces 节顺序收集动作名并去重（保序：先出现者保留）。
fn push_action(list: &mut Vec<String>, name: &str) {
  if !list.iter().any(|x| x == name) {
    list.push(name.to_string());
  }
}

/// pattern → 动作名：press(Invoke) toggle(Toggle) expand/collapse(ExpandCollapse)
/// select(SelectionItem) scroll_into_view(ScrollItem)，追加 LegacyIAccessible
/// DefaultAction 原文——ExpandCollapse 两个动作都列出（pattern 支持的操作，与状态无关）。
fn actions_of(el: &IUIAutomationElement) -> Vec<String> {
  let mut out: Vec<String> = Vec::new();
  if unsafe { el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) }.is_ok() {
    push_action(&mut out, "press");
  }
  if unsafe { el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId) }.is_ok() {
    push_action(&mut out, "toggle");
  }
  if unsafe {
    el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId)
  }
  .is_ok()
  {
    push_action(&mut out, "expand");
    push_action(&mut out, "collapse");
  }
  if unsafe {
    el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
  }
  .is_ok()
  {
    push_action(&mut out, "select");
  }
  if unsafe { el.GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(UIA_ScrollItemPatternId) }
    .is_ok()
  {
    push_action(&mut out, "scroll_into_view");
  }
  if let Ok(legacy) = unsafe {
    el.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
      UIA_LegacyIAccessiblePatternId,
    )
  } {
    if let Ok(default_action) = unsafe { legacy.CurrentDefaultAction() } {
      let text = default_action.to_string();
      if !text.is_empty() {
        push_action(&mut out, &text);
      }
    }
  }
  out
}

/// ValuePattern.CurrentValue；无 pattern 或读取失败 → None。
fn value_of(el: &IUIAutomationElement) -> Option<String> {
  let pattern =
    unsafe { el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }.ok()?;
  unsafe { pattern.CurrentValue() }.ok().map(|v| v.to_string())
}

/// RuntimeId：GetRuntimeId 返回调用方持有的 SAFEARRAY（一维 VT_I4），读完必须销毁。
fn runtime_id(el: &IUIAutomationElement) -> Option<Vec<i32>> {
  unsafe {
    let array = el.GetRuntimeId().ok()?;
    let mut data: *mut c_void = std::ptr::null_mut();
    let mut out = None;
    if SafeArrayAccessData(array, &mut data).is_ok() {
      if (*array).cDims >= 1 {
        let count = (*array).rgsabound[0].cElements as usize;
        out = Some(std::slice::from_raw_parts(data as *const i32, count).to_vec());
      }
      let _ = SafeArrayUnaccessData(array);
    }
    let _ = SafeArrayDestroy(array);
    out
  }
}
