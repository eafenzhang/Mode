//! 二进制层的端到端断言：契约 A 的横幅与契约 B 的**校验路径**。
//!
//! 校验发生在提权之前，因此这些用例无需 UAC、无需真实 Chrome，可在任何已构建
//! 产物的平台上跑（`CARGO_BIN_EXE_*` 由 cargo 注入）。
use std::io::Write;
use std::process::{Command, Stdio};

fn run(args: &[&str], stdin: &str) -> (i32, String) {
    let exe = env!("CARGO_BIN_EXE_mode-browser-import-helper");
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn helper");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

#[test]
fn version_flag_prints_frozen_banner() {
    let (code, stdout) = run(&["--version"], "");
    assert_eq!(code, 0);
    let fields: Vec<&str> = stdout.trim().split('\t').collect();
    assert_eq!(fields.len(), 5);
    assert_eq!(fields[0], "ZCODE_BROWSER_IMPORT_HELPER");
    assert_eq!(fields[1], "2");
}

#[test]
fn no_args_or_unknown_args_exit_two_without_stdout() {
    for args in [vec![], vec!["--help"], vec!["--nope"]] {
        let (code, stdout) = run(&args, "");
        assert_eq!(code, 2, "args={args:?}");
        assert!(stdout.is_empty(), "args={args:?} stdout={stdout:?}");
    }
}

#[test]
fn malformed_broker_request_emits_validation_error_frame() {
    let (code, stdout) = run(&["--broker", "--parent-pid", "1"], "garbage\n");
    assert_eq!(code, 0);
    assert_eq!(
        stdout.trim(),
        "ZCODE_BROWSER_IMPORT_HELPER\tERR\tvalidation_failed"
    );
}

#[test]
fn well_formed_request_fails_closed_without_forging_a_frame() {
    // 帧合法但 P2 未落地：必须完全不产出 stdout，不能伪造 OK 或 ERR 帧。
    // 载荷内容不影响校验（只要求 base64 合法且非空），故用最小合法值。
    let request = "ZCODE_BROWSER_IMPORT_HELPER\tBwcHBw==\tQQ==\n";
    let (code, stdout) = run(&["--broker", "--parent-pid", "1"], request);
    assert_ne!(code, 0);
    assert!(stdout.is_empty(), "stdout={stdout:?}");
}
