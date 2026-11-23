//! `remote-desktop` 的两块纯逻辑：显示器校验 + 编码能力持久化。
//!
//! 真源：
//! - `packages/desktop/src/host-standalone/remoteDesktopService.ts::setLocalDisplays`
//! - `packages/desktop/src/host/encoderCapabilityStore.ts`
//!
//! 规范见 `docs/specs/remote-desktop-session.md` §9。
//!
//! 两块都不依赖媒体面，所以可以先落：显示器校验是纯函数，编码能力是「读 + 写 + 从
//! start 结果映射」三步。

use std::path::Path;

use serde_json::{json, Value as Json};

/// 编码能力（`RemoteDesktopEncoderCapability`）。
pub const ENCODER_UNKNOWN: &str = "unknown";
pub const ENCODER_AVAILABLE: &str = "available";
pub const ENCODER_SOFTWARE: &str = "software";
pub const ENCODER_UNAVAILABLE: &str = "unavailable";

/// 本地显示器（`RemoteDesktopLocalDisplay` 的规范化形态）。
#[derive(Debug, Clone, PartialEq)]
pub struct LocalDisplay {
    pub display_id: String,
    pub label: String,
    pub x_px: i64,
    pub y_px: i64,
    pub width_px: i64,
    pub height_px: i64,
    pub scale_factor: f64,
    pub primary: bool,
}

/// 校验并规范化显示器列表（TS `setLocalDisplays` 的 7 条过滤 + 6 条归一）。
///
/// 过滤（任一不满足即丢弃该条）：
/// 1. 是对象；2. `displayId` 是非空字符串；3. `xPx` 有限；4. `yPx` 有限；
/// 5. `widthPx` 有限；6. `heightPx` 有限；7. 宽高都 > 0。
///
/// 归一：`displayId` trim、`label` 非字符串时给空串、坐标 `trunc`、宽高至少 1、
/// `scaleFactor` 非有限或 ≤0 时取 1、`primary` 只在严格 `true` 时为真。
///
/// **兜底**：非空列表里若一个 primary 都没有，第一条置为 primary
/// （否则对端没有可投的主显示器）。
pub fn sanitize_local_displays(raw: &Json) -> Vec<LocalDisplay> {
    let Some(items) = raw.as_array() else {
        return Vec::new();
    };
    let mut displays: Vec<LocalDisplay> = items
        .iter()
        .filter_map(|item| {
            let object = item.as_object()?;
            let display_id = object.get("displayId")?.as_str()?.trim();
            if display_id.is_empty() {
                return None;
            }
            let x_px = finite_number(object.get("xPx"))?;
            let y_px = finite_number(object.get("yPx"))?;
            let width_px = finite_number(object.get("widthPx"))?;
            let height_px = finite_number(object.get("heightPx"))?;
            if width_px <= 0.0 || height_px <= 0.0 {
                return None;
            }
            let scale_factor = object
                .get("scaleFactor")
                .and_then(Json::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0)
                .unwrap_or(1.0);
            Some(LocalDisplay {
                display_id: display_id.to_string(),
                label: object
                    .get("label")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string(),
                x_px: x_px.trunc() as i64,
                y_px: y_px.trunc() as i64,
                width_px: (width_px.trunc() as i64).max(1),
                height_px: (height_px.trunc() as i64).max(1),
                scale_factor,
                primary: object.get("primary").and_then(Json::as_bool) == Some(true),
            })
        })
        .collect();
    if !displays.is_empty() && !displays.iter().any(|display| display.primary) {
        displays[0].primary = true;
    }
    displays
}

fn finite_number(value: Option<&Json>) -> Option<f64> {
    let number = value?.as_f64()?;
    number.is_finite().then_some(number)
}

/// 显示器列表 → 线上形状（供端口下发/日志）。
pub fn local_displays_to_json(displays: &[LocalDisplay]) -> Json {
    Json::Array(
        displays
            .iter()
            .map(|display| {
                json!({
                    "displayId": display.display_id,
                    "label": display.label,
                    "xPx": display.x_px,
                    "yPx": display.y_px,
                    "widthPx": display.width_px,
                    "heightPx": display.height_px,
                    "scaleFactor": display.scale_factor,
                    "primary": display.primary,
                })
            })
            .collect(),
    )
}

/// `rd_media_start` 结果 → 能力取值（TS `encoderCapabilityFromStartResult`）。
///
/// helper 回 `encoder:"software"` = 本机无硬件编码器、已回退系统软件编码（能出画面，
/// 但要给代价提示）；其余（`"hardware"`、字段缺失、形状异常）一律 `available`
/// ——**不猜、不误报失败**：会话已经建流成功，能力面就只能是「可用」这一档。
pub fn encoder_capability_from_start_result(result: &Json) -> &'static str {
    match result.get("encoder").and_then(Json::as_str) {
        Some("software") => ENCODER_SOFTWARE,
        _ => ENCODER_AVAILABLE,
    }
}

/// 读缓存：缺失/损坏/形状非法一律 `None`（下次真实会话观测后写回）。
///
/// 与频道里的 `load_encoder_capability`（返回 `unknown` 供 UI）区分：这里返回
/// `Option`，让调用方能区分「没有缓存」与「缓存就是 unknown」。
pub fn load_persisted_encoder_capability(path: &Path) -> Option<&'static str> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Json = serde_json::from_str(&text).ok()?;
    sanitize_stored_encoder_capability(&value)
}

/// 白名单收敛：非对象 / 未知枚举值 → `None`（无缓存）。
pub fn sanitize_stored_encoder_capability(raw: &Json) -> Option<&'static str> {
    match raw.get("capability").and_then(Json::as_str) {
        Some("available") => Some(ENCODER_AVAILABLE),
        Some("software") => Some(ENCODER_SOFTWARE),
        Some("unavailable") => Some(ENCODER_UNAVAILABLE),
        Some("unknown") => Some(ENCODER_UNKNOWN),
        _ => None,
    }
}

/// 写缓存（目录不存在时自建；`observedAt` 用调用方给的毫秒时间戳）。
///
/// 写入失败只回错误、不 panic：调用方按 fire-and-forget 处理（写失败不影响会话）。
pub fn persist_encoder_capability(
    path: &Path,
    capability: &str,
    observed_at_ms: i64,
) -> Result<(), String> {
    // 只允许白名单值落盘，避免把未知字符串写进缓存后又被读侧丢弃。
    if sanitize_stored_encoder_capability(&json!({ "capability": capability })).is_none() {
        return Err(format!("未知的编码能力取值：{capability}"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let payload = json!({ "capability": capability, "observedAt": observed_at_ms });
    std::fs::write(path, payload.to_string()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "rd-encoder-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos() as u64)
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("dir");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn displays_drop_entries_that_fail_the_seven_filters() {
        let raw = json!([
            { "displayId": "", "xPx": 0, "yPx": 0, "widthPx": 1, "heightPx": 1 },
            { "displayId": "ok", "xPx": 0, "yPx": 0, "widthPx": 0, "heightPx": 1 },
            { "displayId": "ok2", "xPx": 0, "yPx": 0, "widthPx": 1, "heightPx": 0 },
            { "displayId": "ok3", "widthPx": 10, "heightPx": 10 },
            { "displayId": "ok4", "xPx": 0, "yPx": 0, "widthPx": 10, "heightPx": 10 },
        ]);
        let displays = sanitize_local_displays(&raw);
        assert_eq!(displays.len(), 1, "只有最后一条通过全部过滤");
        assert_eq!(displays[0].display_id, "ok4");
    }

    #[test]
    fn displays_are_normalized_per_ts() {
        let raw = json!([{
            "displayId": "  disp-1  ",
            "xPx": 1.9,
            "yPx": -2.9,
            "widthPx": 1920.7,
            "heightPx": 1080.2,
            "scaleFactor": 0,
            "primary": "yes",
            "label": 42,
        }]);
        let displays = sanitize_local_displays(&raw);
        assert_eq!(displays.len(), 1);
        let display = &displays[0];
        assert_eq!(display.display_id, "disp-1", "displayId 要 trim");
        assert_eq!(display.x_px, 1, "坐标 trunc（向零取整）");
        assert_eq!(display.y_px, -2, "负数向零取整，不是向下取整");
        assert_eq!(display.width_px, 1920);
        assert_eq!(display.height_px, 1080);
        assert_eq!(display.scale_factor, 1.0, "非法 scaleFactor 回落 1");
        assert_eq!(display.label, "", "非字符串 label 给空串");
        // `primary` 只在严格 true 时为真；但兜底会把第一条置为 primary。
        assert!(display.primary, "非空列表必须有 primary");
    }

    #[test]
    fn first_display_becomes_primary_when_none_is_marked() {
        let raw = json!([
            { "displayId": "a", "xPx": 0, "yPx": 0, "widthPx": 10, "heightPx": 10 },
            { "displayId": "b", "xPx": 0, "yPx": 0, "widthPx": 10, "heightPx": 10, "primary": true },
        ]);
        let displays = sanitize_local_displays(&raw);
        assert!(!displays[0].primary, "已有 primary 时不抢");
        assert!(displays[1].primary);

        let raw = json!([
            { "displayId": "a", "xPx": 0, "yPx": 0, "widthPx": 10, "heightPx": 10 },
            { "displayId": "b", "xPx": 0, "yPx": 0, "widthPx": 10, "heightPx": 10 },
        ]);
        let displays = sanitize_local_displays(&raw);
        assert!(displays[0].primary, "没有 primary 时把第一条顶上");
        assert!(!displays[1].primary);
    }

    #[test]
    fn non_array_input_yields_empty_list() {
        assert!(sanitize_local_displays(&json!(null)).is_empty());
        assert!(sanitize_local_displays(&json!({})).is_empty());
        assert!(
            sanitize_local_displays(&json!([
                { "displayId": "a", "xPx": 0, "yPx": 0, "widthPx": 10, "heightPx": 10 }
            ]))
            .len()
                == 1
        );
    }

    #[test]
    fn start_result_maps_software_only_for_the_explicit_value() {
        assert_eq!(
            encoder_capability_from_start_result(&json!({ "encoder": "software" })),
            ENCODER_SOFTWARE
        );
        assert_eq!(
            encoder_capability_from_start_result(&json!({ "encoder": "hardware" })),
            ENCODER_AVAILABLE
        );
        // 字段缺失/形状异常都不误报失败：会话已建流成功，能力只能是「可用」。
        assert_eq!(
            encoder_capability_from_start_result(&json!({})),
            ENCODER_AVAILABLE
        );
        assert_eq!(
            encoder_capability_from_start_result(&json!(null)),
            ENCODER_AVAILABLE
        );
        assert_eq!(
            encoder_capability_from_start_result(&json!({ "encoder": 7 })),
            ENCODER_AVAILABLE
        );
    }

    #[test]
    fn persist_and_load_round_trip_whitelists_values() {
        let dir = TempDir::new("roundtrip");
        let path = dir.0.join("v2").join("rd-encoder-capability.json");

        assert_eq!(load_persisted_encoder_capability(&path), None, "文件不存在");

        persist_encoder_capability(&path, ENCODER_SOFTWARE, 1_700_000_000_000).expect("写");
        assert_eq!(
            load_persisted_encoder_capability(&path),
            Some(ENCODER_SOFTWARE)
        );

        // 落盘形状带 observedAt（跨进程保留观测时间）。
        let raw: Json =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(raw["capability"], ENCODER_SOFTWARE);
        assert_eq!(raw["observedAt"], 1_700_000_000_000_i64);

        // 未知取值拒绝写（否则写进去后读侧又会丢弃，形成静默不一致）。
        assert!(persist_encoder_capability(&path, "turbo", 0).is_err());
        assert_eq!(
            load_persisted_encoder_capability(&path),
            Some(ENCODER_SOFTWARE),
            "拒绝写之后文件内容不变"
        );

        // 损坏文件按无缓存处理，不影响运行。
        std::fs::write(&path, "{broken").expect("write");
        assert_eq!(load_persisted_encoder_capability(&path), None);
    }
}
