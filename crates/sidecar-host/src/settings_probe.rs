//! 壳在 **WebView/Host 起来之前**就要消费的设置探针（当前：硬件加速开关、数据基目录指针）。
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

/// 从 setting.json 原文解析持久化的数据基目录指针 `dataBaseDir`
/// （TS 侧 `persistedDataBaseDirFromSettings` 的逐字对应）：坏 JSON / 缺键 / 非字符串 /
/// 空串一律 `None`，回落家目录。指针只在 `MODE_DATA_BASE_DIR` 未设置时生效——
/// 优先级与 Host 侧 `resolveDataRoot` + `withPersistedDataBaseDir` 一致。
pub fn persisted_data_base_dir_from_settings(raw: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    let dir = value.get("dataBaseDir")?.as_str()?.trim();
    (!dir.is_empty()).then(|| dir.to_string())
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
    fn persisted_pointer_is_trimmed() {
        assert_eq!(
            persisted_data_base_dir_from_settings(r#"{"dataBaseDir":"  D:/ModeData  "}"#),
            Some("D:/ModeData".to_string())
        );
    }

    #[test]
    fn missing_or_unusable_pointer_falls_back() {
        assert_eq!(persisted_data_base_dir_from_settings("{}"), None);
        assert_eq!(
            persisted_data_base_dir_from_settings(r#"{"dataBaseDir":"   "}"#),
            None
        );
        assert_eq!(
            persisted_data_base_dir_from_settings(r#"{"dataBaseDir":42}"#),
            None
        );
        assert_eq!(persisted_data_base_dir_from_settings("{:wq}"), None);
        assert_eq!(persisted_data_base_dir_from_settings(""), None);
    }

    #[test]
    fn default_root_settings_file_is_home_mode_v2() {
        let file = default_root_settings_file(std::path::Path::new("/home/u"));
        assert_eq!(file, std::path::Path::new("/home/u/.mode/v2/setting.json"));
    }
}
