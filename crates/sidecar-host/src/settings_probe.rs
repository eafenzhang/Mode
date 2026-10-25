//! 壳在 **WebView/Host 起来之前**就要消费的设置探针（当前：硬件加速开关）。
//!
//! 为什么放本 crate：判定是纯函数、要单测；`mode-tauri` 的测试目标在本地 gnu 工具链
//! 链不起来（见 2026-10-25 验收记录），把可测面放在这里让 CI 与本地都跑得到。
//!
//! 数据源是「默认根引导文件」`~/.mode/v2/setting.json`：它恒在用户主目录、不随数据
//! 目录迁移（`docs/specs/tauri-renderer-host.md` §23 的同一条口径），Host 与壳无需
//! 先行握手即可各自读取。壳只取自己执行的键，不解释业务存储。

/// `~/.mode/v2/setting.json`（默认根引导文件）。
pub fn default_root_settings_file(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".mode").join("v2").join("setting.json")
}

/// 从 setting.json 原文解析 `desktopChromiumHardwareAccelerationEnabled` 是否显式 false。
/// 缺键 = 默认（启用硬件加速）；坏 JSON 按缺键处理——诊断类开关不该因文件损坏
/// 把用户的 GPU 关掉（fail-open 到默认行为，与 Host 侧坏文件容忍同口径）。
pub fn gpu_disabled_from_settings(raw: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    value
        .get("desktopChromiumHardwareAccelerationEnabled")
        .is_some_and(|flag| flag.as_bool() == Some(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_false_disables_gpu() {
        assert!(gpu_disabled_from_settings(
            r#"{"desktopChromiumHardwareAccelerationEnabled":false}"#
        ));
    }

    #[test]
    fn missing_key_and_true_keep_gpu_on() {
        assert!(!gpu_disabled_from_settings("{}"));
        assert!(!gpu_disabled_from_settings(
            r#"{"desktopChromiumHardwareAccelerationEnabled":true}"#
        ));
    }

    #[test]
    fn broken_json_falls_back_to_enabled() {
        assert!(!gpu_disabled_from_settings("{:wq}"));
        assert!(!gpu_disabled_from_settings(""));
    }

    #[test]
    fn non_bool_value_is_not_a_disable() {
        assert!(!gpu_disabled_from_settings(
            r#"{"desktopChromiumHardwareAccelerationEnabled":"false"}"#
        ));
    }

    #[test]
    fn default_root_settings_file_is_home_mode_v2() {
        let file = default_root_settings_file(std::path::Path::new("/home/u"));
        assert_eq!(file, std::path::Path::new("/home/u/.mode/v2/setting.json"));
    }
}
