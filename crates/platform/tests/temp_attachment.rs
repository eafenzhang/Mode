//! `createTempTextAttachment` 的落盘语义。
//!
//! 规格：`docs/specs/tauri-platform-commands.md` §1 第 12 条。
//! 这里只驱动纯函数 `write_temp_text_attachment_in`，不经过 Tauri 命令层：
//! 命令层只负责选系统临时目录并把错误折成稳定码。

use std::fs;
use std::path::{Path, PathBuf};

use platform::commands::temp_attachment::write_temp_text_attachment_in;
use protocol::{CreateTempTextAttachmentRequest, TempTextMimeType};

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mode-temp-attachment-test-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("建测试目录");
    dir
}

fn request(text: &str, filename: Option<&str>) -> CreateTempTextAttachmentRequest {
    CreateTempTextAttachmentRequest {
        text: text.to_string(),
        filename: filename.map(str::to_string),
    }
}

#[test]
fn writes_text_and_reports_the_contract_shape() {
    let root = root("shape");
    let text = "line one\nline two\n";
    let result =
        write_temp_text_attachment_in(&root, &request(text, Some("paste.txt"))).expect("应能写入");

    assert_eq!(result.filename, "paste.txt");
    assert_eq!(result.mime_type, TempTextMimeType::TextPlain);
    assert_eq!(result.size_bytes, text.len() as u64);
    assert!(
        result.local_path.ends_with("paste.txt"),
        "localPath 应指向写入的文件：{}",
        result.local_path
    );
    assert!(
        Path::new(&result.local_path).starts_with(&root),
        "落点必须在给定根目录下：{}",
        result.local_path
    );
    assert_eq!(fs::read_to_string(&result.local_path).expect("读回"), text);

    fs::remove_dir_all(&root).expect("清理");
}

#[test]
fn strips_path_components_from_the_filename() {
    let root = root("traversal");
    let result =
        write_temp_text_attachment_in(&root, &request("x", Some("..\\nested/dir/evil.txt")))
            .expect("应能写入");

    assert_eq!(result.filename, "evil.txt", "只保留最后一段文件名");
    assert!(
        !result.local_path.contains("nested"),
        "路径分量不得进入落点：{}",
        result.local_path
    );

    fs::remove_dir_all(&root).expect("清理");
}

#[test]
fn replaces_characters_that_are_invalid_on_windows() {
    let root = root("invalid");
    let result = write_temp_text_attachment_in(&root, &request("x", Some("a<b>c:d\"e|f?g*.txt")))
        .expect("应能写入");

    assert_eq!(result.filename, "a_b_c_d_e_f_g_.txt");

    fs::remove_dir_all(&root).expect("清理");
}

#[test]
fn falls_back_to_attachment_txt_for_empty_or_dot_names() {
    let root = root("fallback");
    for filename in [None, Some(""), Some("   "), Some("."), Some("..")] {
        let result =
            write_temp_text_attachment_in(&root, &request("x", filename)).expect("应能写入");
        assert_eq!(result.filename, "attachment.txt", "输入 {filename:?}");
    }

    fs::remove_dir_all(&root).expect("清理");
}

#[test]
fn same_filename_gets_distinct_paths() {
    let root = root("collision");
    let first = write_temp_text_attachment_in(&root, &request("first", Some("same.txt")))
        .expect("第一次写入");
    let second = write_temp_text_attachment_in(&root, &request("second", Some("same.txt")))
        .expect("第二次写入");

    assert_ne!(first.local_path, second.local_path, "同名附件不得互相覆盖");
    assert_eq!(
        fs::read_to_string(&first.local_path).expect("读回 first"),
        "first"
    );
    assert_eq!(
        fs::read_to_string(&second.local_path).expect("读回 second"),
        "second"
    );

    fs::remove_dir_all(&root).expect("清理");
}
