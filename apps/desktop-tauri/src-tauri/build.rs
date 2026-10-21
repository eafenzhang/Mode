fn main() {
    // `tauri::test` 的 mock app 经 tao 静态导入 comctl32 的 TaskDialogIndirect。该导出只在
    // Common-Controls v6（需应用清单）里存在；测试二进制默认没有清单，Windows 会加载
    // comctl32 5.82 并在启动时以 STATUS_ENTRYPOINT_NOT_FOUND 退出。只给测试目标嵌入清单。
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=test-manifest.xml");
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("test-manifest.xml")
                .display()
        );
    }
    tauri_build::build();
}
