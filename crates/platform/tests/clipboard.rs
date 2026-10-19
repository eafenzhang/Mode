//! `rdReadClipboardText` 的冒烟用例：命令能在真实进程里调用且不 panic。
//!
//! **不写入剪贴板**（那会改用户的剪贴板内容）：只断言返回值形状——Windows 上是一次真实
//! 的 CF_UNICODETEXT 读取（剪贴板里没有文本时为空串），其它平台恒为空串。
//! 更完整的语义（读到已知文本）需要人工/真机，不在自动化里伪造。
use platform::commands::clipboard::rd_read_clipboard_text;

#[test]
fn reads_text_without_panicking() {
    let result = rd_read_clipboard_text();
    assert!(result.is_ok(), "读取剪贴板不得失败：{result:?}");
    // Windows 上剪贴板被占用时也返回空串（best-effort）；非 Windows 恒为空串。
    let _ = result.unwrap();
}
