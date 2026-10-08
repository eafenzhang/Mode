mod common;

use mode_cua_ax::*;

#[test]
fn observe_indexes_fixture_controls() {
  let fix = common::spawn_fixture();
  let r = observe::observe(fix.hwnd as u32, 3000).expect("observe");
  assert!(r.elements.iter().any(|e| e.kind == "button"));
  assert!(r.elements.iter().any(|e| e.kind == "edit"));
  assert!(r
    .elements
    .iter()
    .any(|e| e.actions.contains(&"press".to_string())));
  assert!(r.elements.iter().all(|e| e.bounds.len() == 4));
  // 强化断言（brief 之外，基于 Week-0 探针对夹具的实测行为）：
  assert_eq!(r.window_title.as_deref(), Some("fixture"));
  assert!(r.enumeration_complete);
  // focusedIndex 要么不匹配（None），要么落在返回集合内。
  assert!(r
    .focused_index
    .is_none_or(|i| (i as usize) < r.elements.len()));
  // EDIT 控件的 ValuePattern 暴露初值。
  let edit = r
    .elements
    .iter()
    .find(|e| e.kind == "edit")
    .expect("edit element");
  assert_eq!(edit.value.as_deref(), Some("hello"));
  fix.destroy();
}

#[test]
fn observe_truncates_at_max_elements() {
  let fix = common::spawn_fixture();
  // 夹具树至少有 window+edit+button 三个元素（探针实测 9 个），2 个上限必截断。
  let r = observe::observe(fix.hwnd as u32, 2).expect("observe");
  assert!(r.elements.len() <= 2);
  assert!(!r.enumeration_complete);
  fix.destroy();
}
