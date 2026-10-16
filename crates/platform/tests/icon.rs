//! `getApplicationIcon` 的契约形状断言（M3 Task 4 · DR-T4-2）。
//!
//! 规格：`docs/specs/tauri-platform-commands.md` 的 `getApplicationIcon` 一节。
//!
//! **这里能自动化的边界**（计划的风险表已登记）：无人值守环境（CI）没有真桌面，
//! 「某个 exe 的图标真的取出来了」只有真机能验。所以本文件是两层：
//!
//! 1. **契约形状**（默认跑，进 CI）：`null` 的取值路径不抛异常、非 http 面以外的 locator
//!    一律安全跳过、编码器产出的是**真 PNG 容器**的 `data:image/png;base64,…`。
//! 2. **真机探针**（opt-in：`MODE_TAURI_ICON_PROBE=1`）：对系统里真实存在的 exe 走完整原生
//!    解码路径，断言拿到非 null 且是 PNG。原始输出入仓（见 spec 的证据登记）。
//!
//! 为什么把「编码器」单独测：它是**契约形状的唯一载体**（前缀 + 真 PNG），而这部分不依赖
//! Shell/GDI，任何机器上都能跑——所以它不该和「有没有桌面」捆在一起。
use protocol::{ApplicationIconLocator, ApplicationIconRequest};

/// 不可解析的 locator：路径不存在、空串、以及只带 macOS bundle id。
fn unresolvable_locators() -> Vec<ApplicationIconLocator> {
    vec![
        ApplicationIconLocator::WindowsExecutablePath {
            value: r"C:\definitely-not-installed\mode-icon-probe.exe".to_string(),
        },
        ApplicationIconLocator::WindowsAumid {
            value: "Mode.NotInstalled.App.1".to_string(),
        },
        ApplicationIconLocator::WindowsExecutablePath {
            value: "   ".to_string(),
        },
        ApplicationIconLocator::DarwinBundleId {
            value: "com.example.not-installed".to_string(),
        },
    ]
}

#[test]
fn unresolvable_locators_yield_null_without_throwing() {
    // 契约：取不到就是 `null`（`Promise<ApplicationIconInfo | null>`），不是错误、不是空串。
    let result = platform::commands::icon::get_application_icon(ApplicationIconRequest {
        locators: unresolvable_locators(),
    });
    assert!(
        result.is_none(),
        "取不到图标时必须返回 null（契约允许且调用方已处理），实际 {result:?}"
    );
}

#[test]
fn an_empty_locator_list_yields_null() {
    let result = platform::commands::icon::get_application_icon(ApplicationIconRequest {
        locators: Vec::new(),
    });
    assert!(result.is_none(), "没有 locator 可试时必须返回 null");
}

#[test]
fn a_macos_only_locator_is_skipped_on_windows_instead_of_erroring() {
    // macOS 半边本阶段未实现（Ruling 20）：在 Windows 上它必须是**安全跳过**，
    // 而不是抛错——渲染层拿到的永远是契约内的取值。
    let result = platform::commands::icon::get_application_icon(ApplicationIconRequest {
        locators: vec![ApplicationIconLocator::DarwinBundleId {
            value: "com.apple.Safari".to_string(),
        }],
    });
    assert!(result.is_none(), "macOS bundle id 在 Windows 上只能是 null");
}

#[cfg(target_os = "windows")]
mod windows_shape {
    use super::*;
    use platform::icon_windows;

    #[test]
    fn an_existing_file_resolves_and_a_missing_one_does_not() {
        // 解析面：存在的文件直接命中（不查注册表），不存在的返回 None。
        let existing = std::env::current_exe().expect("测试进程自己的路径必须拿得到");
        let existing = existing.to_string_lossy().into_owned();
        assert_eq!(
            icon_windows::resolve_executable(&ApplicationIconLocator::WindowsExecutablePath {
                value: existing.clone(),
            }),
            Some(existing),
            "存在的文件必须直接解析成功"
        );

        for value in [
            r"C:\definitely-not-installed\mode-icon-probe.exe",
            "",
            "   ",
        ] {
            assert_eq!(
                icon_windows::resolve_executable(&ApplicationIconLocator::WindowsExecutablePath {
                    value: value.to_string(),
                }),
                None,
                "「{value}」不该解析出路径"
            );
        }
    }

    #[test]
    fn the_encoder_produces_a_real_png_data_url() {
        // 2x2 的 RGBA：四个角各一种颜色（含半透明），编码后逐字节回读。
        let rgba: Vec<u8> = vec![
            255, 0, 0, 255, // 红
            0, 255, 0, 128, // 半透明绿
            0, 0, 255, 0, // 全透明蓝
            255, 255, 255, 255, // 白
        ];
        let data_url = icon_windows::data_url_from_rgba(2, 2, &rgba).expect("编码必须成功");
        let encoded = data_url
            .strip_prefix(icon_windows::PNG_DATA_URL_PREFIX)
            .unwrap_or_else(|| panic!("data URL 的前缀必须是 {}：{data_url}", icon_windows::PNG_DATA_URL_PREFIX));

        use base64::Engine as _;
        let png_bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("前缀之后必须是合法 base64");

        // PNG 签名（8 字节）+ IHDR：宽高各 4 字节大端（偏移 16..24）。
        assert_eq!(
            &png_bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            "载荷必须是真 PNG（调用方把它塞进 <img src>，坏容器会静默显示不出来）"
        );
        assert_eq!(png_bytes[12..16], *b"IHDR", "第一个块必须是 IHDR");
        assert_eq!(
            u32::from_be_bytes(png_bytes[16..20].try_into().unwrap()),
            2,
            "IHDR 里的宽必须与入参一致"
        );
        assert_eq!(
            u32::from_be_bytes(png_bytes[20..24].try_into().unwrap()),
            2,
            "IHDR 里的高必须与入参一致"
        );
    }

    #[test]
    fn the_encoder_rejects_a_buffer_that_does_not_match_the_dimensions() {
        // 形状不对就返回 None（不给调用方一张「看起来像图标」的坏图）。
        assert!(icon_windows::data_url_from_rgba(2, 2, &[0u8; 8]).is_none());
        assert!(icon_windows::data_url_from_rgba(0, 2, &[]).is_none());
    }
}

/// **opt-in 真机探针**：对系统里真实存在的 exe 走完整原生解码。
///
/// 跑法与判据见 spec 的证据登记；默认 `return`（并打一行说明，避免读成「跑了但没输出」）。
#[cfg(target_os = "windows")]
#[test]
fn real_icon_probe_on_a_system_executable() {
    if std::env::var("MODE_TAURI_ICON_PROBE").as_deref() != Ok("1") {
        eprintln!(
            "[icon-probe] 跳过：设 MODE_TAURI_ICON_PROBE=1 才跑（要真 Windows 桌面会话的 Shell）"
        );
        return;
    }
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let notepad = format!(r"{system_root}\System32\notepad.exe");
    let result = platform::commands::icon::get_application_icon(ApplicationIconRequest {
        locators: vec![ApplicationIconLocator::WindowsExecutablePath {
            value: notepad.clone(),
        }],
    });
    let Some(info) = result else {
        panic!("真实存在的 {notepad} 必须取到图标（取不到就是原生解码坏了）");
    };
    println!(
        "[icon-probe] path={notepad} prefix={} base64_bytes={}",
        &info.icon_data_url[.."data:image/png;base64,".len().min(info.icon_data_url.len())],
        info.icon_data_url.len()
    );
    assert!(
        info.icon_data_url.starts_with("data:image/png;base64,"),
        "真实图标的载体必须是 PNG data URL：{}",
        info.icon_data_url
    );
}
