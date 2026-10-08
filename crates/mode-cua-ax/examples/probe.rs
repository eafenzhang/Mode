//! Week-0 探针：打印 observe 全树 JSON（stdout 为纯 JSON，摘要在 stderr）。
//!
//! 用法：
//!   cargo run --example probe -- list      # 枚举候选窗口（本 crate 的 list_windows）
//!   cargo run --example probe -- fixture   # 对进程内 Win32 夹具窗口 observe
//!   cargo run --example probe -- <hwnd十进制>
#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
  match std::env::args().nth(1).as_deref() {
    Some("list") => list_mode(),
    Some("fixture") => {
      let fix = common::spawn_fixture();
      print_observe(fix.hwnd as u32);
      fix.destroy();
    }
    Some(hwnd) => match hwnd.parse::<u32>() {
      Ok(id) => print_observe(id),
      Err(_) => usage(),
    },
    None => usage(),
  }
}

fn usage() -> ! {
  eprintln!("usage: probe list | probe fixture | probe <hwnd-decimal>");
  std::process::exit(2);
}

/// 用本 crate 的 list_windows 枚举候选窗口（controller 要求自举枚举选目标）。
fn list_mode() {
  let apps = mode_cua_ax::apps::list_apps().expect("list_apps");
  let wins = mode_cua_ax::apps::list_windows(None).expect("list_windows");
  for w in wins {
    let proc = apps
      .iter()
      .find(|a| a.pid == w.pid)
      .and_then(|a| a.name.clone())
      .unwrap_or_else(|| "?".into());
    println!(
      "hwnd={} pid={} proc={} title={:?}",
      w.window_id, w.pid, proc, w.title
    );
  }
}

fn print_observe(window_id: u32) {
  let max = mode_cua_ax::observe::DEFAULT_MAX_ELEMENTS;
  match mode_cua_ax::observe::observe(window_id, max) {
    Ok(result) => {
      println!(
        "{}",
        serde_json::to_string_pretty(&result).expect("serialize observe result")
      );
      // 人类可读摘要（stderr，stdout 保持纯 JSON）。
      let mut by_kind: std::collections::BTreeMap<&str, usize> = Default::default();
      let mut with_actions = 0usize;
      for e in &result.elements {
        *by_kind.entry(e.kind.as_str()).or_default() += 1;
        if !e.actions.is_empty() {
          with_actions += 1;
        }
      }
      eprintln!("--- probe summary hwnd={window_id} ---");
      eprintln!("windowTitle={:?}", result.window_title);
      eprintln!(
        "elements={} enumerationComplete={} focusedIndex={:?} withActions={with_actions}",
        result.elements.len(),
        result.enumeration_complete,
        result.focused_index
      );
      eprintln!("kinds={by_kind:?}");
    }
    Err(e) => {
      eprintln!("observe failed: code={} message={}", e.code, e.message);
      std::process::exit(1);
    }
  }
}
