//! 给**测试目标**注入 Windows 应用清单（comctl32 v6 依赖）。
//!
//! ## 为什么需要
//!
//! tao/tauri 的 Windows 路径会用到 comctl32 **v6** 才有的导出（`TaskDialogIndirect` 等）。
//! v6 不是默认加载的——只有可执行文件带应用清单、声明 `Microsoft.Windows.Common-Controls`
//! 6.0.0.0 依赖时才会启用；否则加载到 v5，进程在启动阶段直接
//! `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)`。app 二进制（`apps/desktop-tauri`）的清单由
//! `tauri_build::build()` 注入，但 **cargo 的测试二进制不会**，于是 `cargo test -p platform` 会挂。
//!
//! ## 为什么不用 tauri-winres / embed-resource
//!
//! 两个原因，都会直接破坏构建：
//!
//! 1. **传播**：本 crate 没有 `[[bin]]`，它们会走「`rustc-link-search` + `rustc-link-lib=resource`」
//!    的兼容分支，而 link-lib 会**传播到依赖方** —— 清单因此被链进 app 二进制，与 tauri-build
//!    那份撞车（`duplicate resource: type MANIFEST (ID 24)` / `type VERSIONINFO`）。
//! 2. **VERSIONINFO 无法关闭**：`WindowsResource::write_resource_file` 无条件写 `1 VERSIONINFO`，
//!    所以「让 platform 出清单、app 去掉自己的清单」这条路也走不通——版本信息仍会重复。
//!
//! ## 做法
//!
//! 只发 `rustc-link-arg-tests`（按目标类型限定，**不向依赖方传播**），并按目标环境选注入方式：
//!
//! - `msvc`：交给 link.exe 的 `/MANIFESTINPUT:<fragment>`，**不需要任何外部工具**；
//! - 其它（gnu/mingw）：用 `windres` 把 `manifest.rc` 编成 COFF 目标文件再链进去。
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 与 `tauri-build` 的默认清单同内容（`windows-app-manifest.xml`）：只声明 comctl32 v6 依赖。
/// 刻意不带 VERSIONINFO / 图标——那些属于 app 二进制，由 `tauri_build::build()` 负责。
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    match emit_test_link_args() {
        Ok(()) => {}
        // 找不到资源编译器时只告警不失败：app 构建不需要这份清单（它自带），
        // 测试二进制起不来会直接暴露，不需要在这里把整个构建卡死。
        Err(message) => println!("cargo:warning=platform: 未能注入 comctl32 v6 测试清单（{message}）"),
    }
}

fn emit_test_link_args() -> Result<(), String> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|error| error.to_string())?);
    let manifest = out_dir.join("platform-test-app.manifest");
    fs::write(&manifest, MANIFEST).map_err(|error| error.to_string())?;

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // link.exe 的清单片段合并：`/MANIFESTINPUT` 让链接器把这段依赖并进最终清单。
        // 全程不需要 rc.exe，避免 CI 上「SDK 的 bin 是否在 PATH」这种环境假设。
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
            manifest.display()
        );
        return Ok(());
    }

    // gnu/mingw：资源编译器把 rc 编成 COFF 目标文件，直接当链接输入。
    let rc = out_dir.join("platform-test-app.rc");
    // 资源编译器按 rc 文件所在目录解析相对路径，故写相对文件名并在该目录下执行。
    fs::write(&rc, "1 24 \"platform-test-app.manifest\"\n").map_err(|error| error.to_string())?;
    let object = out_dir.join("platform-test-app.coff.o");
    let tool = find_tool(&[
        format!("{}-windres", target_prefix()),
        "windres".to_string(),
        "llvm-windres".to_string(),
    ])?;
    let status = Command::new(&tool)
        .args([
            "-O",
            "coff",
            "-i",
            "platform-test-app.rc",
            "-o",
            "platform-test-app.coff.o",
        ])
        .current_dir(&out_dir)
        .status()
        .map_err(|error| format!("执行 {tool} 失败：{error}"))?;
    if !status.success() {
        return Err(format!("{tool} 退出码 {status}"));
    }
    println!("cargo:rustc-link-arg-tests={}", object.display());
    Ok(())
}

/// 资源编译器的候选名：先看 `RC` 环境变量（构建环境可显式指定），再按工具链前缀猜。
fn find_tool(candidates: &[String]) -> Result<String, String> {
    if let Ok(explicit) = env::var("RC") {
        if !explicit.trim().is_empty() {
            return Ok(explicit);
        }
    }
    for candidate in candidates {
        if on_path(candidate) {
            return Ok(candidate.clone());
        }
    }
    Err(format!("PATH 上找不到 {candidates:?}"))
}

fn on_path(tool: &str) -> bool {
    if Path::new(tool).is_absolute() {
        return Path::new(tool).exists();
    }
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path).any(|dir| {
        let candidate = dir.join(tool);
        candidate.exists() || candidate.with_extension("exe").exists()
    })
}

/// 目标三元组前缀（如 `x86_64-w64-mingw32`），用于拼 `*-windres`。
fn target_prefix() -> String {
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "aarch64",
        Ok("x86") => "i686",
        _ => "x86_64",
    };
    format!("{arch}-w64-mingw32")
}
