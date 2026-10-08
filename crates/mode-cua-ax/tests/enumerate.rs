mod common;
use mode_cua_ax::*; // 经 napi 导出的 fn 不在 crate 根可直接调，改为把纯逻辑放 pub fn

#[test]
fn list_apps_returns_system_processes() {
  let apps = apps::list_apps().expect("enumerate");
  assert!(!apps.is_empty());
  assert!(apps.iter().any(|a| a.name.is_some()));
}

#[test]
fn list_windows_finds_own_test_window() {
  let fix = common::spawn_fixture();
  let wins = apps::list_windows(None).expect("enumerate");
  assert!(!wins.is_empty());

  // Task 3 回填：夹具就绪后的语义断言（fixture 与本测试同进程，pid = 自身进程）。
  let own_pid = std::process::id();
  let filtered = apps::list_windows(Some(own_pid)).expect("enumerate");
  assert!(!filtered.is_empty());
  assert!(filtered.iter().all(|w| w.pid == own_pid));

  let row = filtered
    .iter()
    .find(|w| w.title.as_deref() == Some("fixture"))
    .expect("fixture window row");
  assert_eq!(row.bounds.len(), 4);
  assert!(row.bounds[2] > 0 && row.bounds[3] > 0);
  fix.destroy();
}
