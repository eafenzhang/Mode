use mode_cua_ax::*; // 经 napi 导出的 fn 不在 crate 根可直接调，改为把纯逻辑放 pub fn

#[test]
fn list_apps_returns_system_processes() {
  let apps = apps::list_apps().expect("enumerate");
  assert!(!apps.is_empty());
  assert!(apps.iter().any(|a| a.name.is_some()));
}
#[test]
fn list_windows_finds_own_test_window() { // Task 3 fixture 就绪后在该任务补全；此处先断言桌面窗口存在
  let wins = apps::list_windows(None).expect("enumerate");
  assert!(!wins.is_empty());
}
